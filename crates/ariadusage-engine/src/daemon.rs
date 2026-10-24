//! Engine daemon runtime, notification boundary, and signal-managed IPC v1 server loop.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use ariadusage_core::config::{Config, CookieSource, ProviderConfig, ProviderEntry};
use ariadusage_core::providers::SourceMode;
use ariadusage_protocol::ids::{ActionId, ProviderId, SettingId};
use ariadusage_protocol::ipc::{ErrorCode, IpcError, ServerMessage};
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
use crate::ipc_server::{IpcBackend, IpcServerError, start_ipc_server};
use crate::paths::{PathError, config_path, socket_path};

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
}

/// Default notifier for the engine daemon.
///
/// On macOS, desktop notifications return [`NotificationError::Unsupported`] in this slice
/// (`UNUserNotificationCenter` lives in a separate UI slice).
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultNotifier;

impl Notifier for DefaultNotifier {
    fn notify(&self, _title: &str, _body: &str) -> Result<(), NotificationError> {
        Err(NotificationError::Unsupported)
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
                .unwrap_or("auto");

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
    let cfg_path = config_path()?;
    let backend: Arc<dyn IpcBackend> = Arc::new(ConfigIpcBackend::new(ConfigStore::new(cfg_path)));

    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    run_daemon_with_shutdown(socket_override, backend, async move {
        tokio::select! {
            _ = sigint.recv() => {}
            _ = sigterm.recv() => {}
        }
    })
    .await
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
