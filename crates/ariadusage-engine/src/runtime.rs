// Ported from CodexBar Sources/CodexBar/UsageStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+Refresh.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+ProviderStatus.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+AdaptiveRefresh.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Engine runtime core: snapshot store, refresh coordinator orchestration, status feed polling,
//! adaptive cadence controller, edge-triggered transition seam, and IPC v1 intent entry points.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use ariadusage_core::config::{Config, QuotaWarningWindow, SecretPresence};
use ariadusage_core::model::{UsageSnapshot, identities_match};
use ariadusage_core::notifications::{
    CodexAccountScopedRefreshGuard, CodexActiveSource, CodexIdentity, CodexLimitResetOwnerKey,
    CodexSessionQuotaOwnerKey, CredentialFetchOutcome, CredentialNotificationAction,
    NotificationPolicyEngine, NotificationPost, ProviderCredentialError, ProviderFetchErrorKind,
};
use ariadusage_core::pace::UsagePace;
use ariadusage_core::pipeline::{
    BoxFuture, FetchContext, FetchError, FetchInteraction, FetchPhase, FetchRuntime, Sleeper,
    error_kind_to_category, fetch_outcome,
};
use ariadusage_core::projection::{
    ProjectedError, ProjectionInput, RefreshState, provider_snapshot,
};
use ariadusage_core::providers::{ProviderDescriptor, SourceMode};
use ariadusage_core::refresh::{
    AdaptiveRefreshDecision, AdaptiveRefreshInput, AdaptiveRefreshPolicyCore,
    BackgroundWorkPowerPolicy, FailureGate, FailureInput, HookDispatch, HookProviderObservation,
    HookProviderStatus, HookQuotaLaneKey, HookQuotaLaneObservation, HookTransitionDetector,
    HooksConfig, RefreshFrequency, SessionQuotaTransition, SessionQuotaTransitionDetector,
    SurfacePolicy, ThermalPressure, failure_decision, hook_account_discriminator,
    hook_account_display_name, is_cancellation, is_preservable_error,
    next_fixed_timer_scheduled_at, on_account_change, should_advance_adaptive_timer,
};
use ariadusage_core::status::StatuspageComponentSummary;
use ariadusage_protocol::ids::{ActionId, ProviderId, SettingId};
use ariadusage_protocol::ipc::{
    ClientMessage, ErrorCode, IpcError, PROTOCOL_V1, ServerMessage, negotiate_protocol,
};
use ariadusage_protocol::metric::{Confidence, Metric, MetricSource, MetricState, SourceKind};
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::settings::{SettingsPage, SettingsScope};
use ariadusage_protocol::snapshot::{EngineInfo, EngineSnapshot, ProviderSnapshot};
use ariadusage_protocol::usage::{ProviderErrorKind, ProviderStatus};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;

use crate::brokers::call::BrokerCall;
use crate::brokers::cookie_cache::CookieCache;
use crate::brokers::net::NetBroker;
use crate::brokers::secret_store::{SecretId, SecretLookup, SecretStore};
use crate::config_store::ConfigStore;
use crate::coordinator::{ProviderRefreshCoordinator, WaitResult};
use crate::daemon::{NoopNotifier, Notifier};
use crate::hooks::{FORWARDED_ENVIRONMENT_KEYS, HookRunner};
use crate::ipc_server::IpcBackend;
use crate::registry::{ProviderPlanInputs, ProviderRegistry, ResolvedProviderSpec};
use crate::settings_page::{
    AppPreferences, SettingsMutationError, apply_setting_mutation, build_settings_page,
};
use crate::status::{ProviderStatusFetcher, StatusFetchError};

/// Default staleness horizon in seconds advertised on `EngineSnapshot`.
pub const DEFAULT_STALE_AFTER_SECONDS: u32 = 300;

/// Tokio-backed [`Sleeper`] used for delayed strategy retries in production runtime runs.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    fn sleep<'a>(&'a self, duration: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            tokio::time::sleep(duration).await;
        })
    }
}

/// Seam for edge-triggered hooks and session quota notifications.
///
/// The first observation for any provider or account only establishes a baseline and never
/// triggers a hook or notification (`HookTransitionDetector`, `SessionQuotaTransitionDetector`,
/// and `NotificationPolicyEngine` enforce this invariant).
pub trait TransitionSink: Send + Sync {
    /// Invoked when a provider state transition matches one or more configured hook events.
    fn on_hook_dispatch(&self, dispatch: &HookDispatch);

    /// Invoked when a provider's primary session quota transitions to `Depleted` or `Restored`.
    fn on_session_quota_transition(
        &self,
        provider: &ProviderId,
        transition: SessionQuotaTransition,
    );

    /// Invoked when a structured notification post is emitted by [`NotificationPolicyEngine`].
    fn on_notification(&self, _post: &NotificationPost) {}
}

/// Default no-op [`TransitionSink`] used when no external notifier or hook runner is attached.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopTransitionSink;

impl TransitionSink for NoopTransitionSink {
    fn on_hook_dispatch(&self, _dispatch: &HookDispatch) {}

    fn on_session_quota_transition(
        &self,
        _provider: &ProviderId,
        _transition: SessionQuotaTransition,
    ) {
    }
}

/// In-memory [`TransitionSink`] that records all edge-triggered hook dispatches, session
/// quota transitions, and notification posts for inspection in tests and diagnostics.
#[derive(Debug, Default)]
pub struct RecordingTransitionSink {
    hook_dispatches: Mutex<Vec<HookDispatch>>,
    session_transitions: Mutex<Vec<(ProviderId, SessionQuotaTransition)>>,
    notifications: Mutex<Vec<NotificationPost>>,
}

impl RecordingTransitionSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn hook_dispatches(&self) -> Vec<HookDispatch> {
        self.hook_dispatches
            .lock()
            .expect("recording sink lock")
            .clone()
    }

    pub fn session_transitions(&self) -> Vec<(ProviderId, SessionQuotaTransition)> {
        self.session_transitions
            .lock()
            .expect("recording sink lock")
            .clone()
    }

    pub fn notifications(&self) -> Vec<NotificationPost> {
        self.notifications
            .lock()
            .expect("recording sink lock")
            .clone()
    }
}

impl TransitionSink for RecordingTransitionSink {
    fn on_hook_dispatch(&self, dispatch: &HookDispatch) {
        self.hook_dispatches
            .lock()
            .expect("recording sink lock")
            .push(dispatch.clone());
    }

    fn on_session_quota_transition(
        &self,
        provider: &ProviderId,
        transition: SessionQuotaTransition,
    ) {
        self.session_transitions
            .lock()
            .expect("recording sink lock")
            .push((provider.clone(), transition));
    }

    fn on_notification(&self, post: &NotificationPost) {
        self.notifications
            .lock()
            .expect("recording sink lock")
            .push(post.clone());
    }
}

/// Thread-safe store holding the latest assembled [`EngineSnapshot`] and broadcasting updates
/// over a [`tokio::sync::watch`] channel for IPC subscribers (`w-mac-3`).
pub struct SnapshotStore {
    tx: watch::Sender<EngineSnapshot>,
    rx: watch::Receiver<EngineSnapshot>,
}

impl std::fmt::Debug for SnapshotStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotStore")
            .field("current", &*self.rx.borrow())
            .finish()
    }
}

impl SnapshotStore {
    /// Creates a new `SnapshotStore` seeded with `initial`.
    pub fn new(initial: EngineSnapshot) -> Self {
        let (tx, rx) = watch::channel(initial);
        Self { tx, rx }
    }

    /// Returns a clone of the latest published [`EngineSnapshot`].
    pub fn current(&self) -> EngineSnapshot {
        self.rx.borrow().clone()
    }

    /// Subscribes to [`EngineSnapshot`] updates via a [`tokio::sync::watch::Receiver`].
    pub fn subscribe(&self) -> watch::Receiver<EngineSnapshot> {
        self.rx.clone()
    }

    /// Publishes a new [`EngineSnapshot`] to all active subscribers.
    pub fn publish(&self, snapshot: EngineSnapshot) {
        let _ = self.tx.send(snapshot);
    }
}

/// Options controlling how a batch or per-provider refresh is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshOptions {
    /// Whether this refresh was initiated by a background timer or an explicit user action.
    pub interaction: FetchInteraction,
    /// Lifecycle phase (`Startup` vs `Regular`).
    pub phase: FetchPhase,
    /// When `true`, joins an already-in-flight refresh for the same provider instead of
    /// cancelling and replacing it.
    pub coalesce_if_refreshing: bool,
    /// Failure surfacing policy (`Gate` by default; hides the first consecutive preservable
    /// failure when prior data exists).
    pub surface_policy: SurfacePolicy,
    /// Whether to poll the provider's status feed alongside usage metrics.
    pub refresh_status: bool,
}

impl Default for RefreshOptions {
    fn default() -> Self {
        Self {
            interaction: FetchInteraction::Background,
            phase: FetchPhase::Regular,
            coalesce_if_refreshing: true,
            surface_policy: SurfacePolicy::Gate,
            refresh_status: true,
        }
    }
}

impl RefreshOptions {
    /// Options for a background timer tick.
    pub const fn background() -> Self {
        Self {
            interaction: FetchInteraction::Background,
            phase: FetchPhase::Regular,
            coalesce_if_refreshing: true,
            surface_policy: SurfacePolicy::Gate,
            refresh_status: true,
        }
    }

    /// Options for a user-initiated manual refresh (`ClientMessage::Refresh`).
    pub const fn user_initiated() -> Self {
        Self {
            interaction: FetchInteraction::UserInitiated,
            phase: FetchPhase::Regular,
            coalesce_if_refreshing: true,
            surface_policy: SurfacePolicy::Gate,
            refresh_status: true,
        }
    }

    /// Options for a required refresh after a settings or account change: cancels and supersedes
    /// any in-flight provider refresh so the new configuration takes effect immediately.
    pub const fn settings_change() -> Self {
        Self {
            interaction: FetchInteraction::UserInitiated,
            phase: FetchPhase::Regular,
            coalesce_if_refreshing: false,
            surface_policy: SurfacePolicy::Gate,
            refresh_status: true,
        }
    }
}

#[derive(Debug, Clone)]
struct ProviderRuntimeState {
    snapshot: Option<UsageSnapshot>,
    last_known_reset_snapshot: Option<UsageSnapshot>,
    account_key: Option<String>,
    failure_gate: FailureGate,
    last_error: Option<ProjectedError>,
    refresh_state: RefreshState,
    kept_snapshot: bool,
    source_kind: SourceKind,
    source_label: String,
    status: Option<ProviderStatus>,
    status_components: Vec<StatuspageComponentSummary>,
    status_as_of: Option<Timestamp>,
    status_request_generation: u64,
    status_published_generation: u64,
    enablement_revision: u64,
}

impl Default for ProviderRuntimeState {
    fn default() -> Self {
        Self {
            snapshot: None,
            last_known_reset_snapshot: None,
            account_key: None,
            failure_gate: FailureGate::new(),
            last_error: None,
            refresh_state: RefreshState::Fresh,
            kept_snapshot: false,
            source_kind: SourceKind::Cli,
            source_label: "none".to_string(),
            status: None,
            status_components: Vec::new(),
            status_as_of: None,
            status_request_generation: 0,
            status_published_generation: 0,
            enablement_revision: 0,
        }
    }
}

#[derive(Debug, Default)]
struct BatchRefreshState {
    is_refreshing: bool,
    is_required_loop_running: bool,
    required_request_generation: u64,
    required_completed_generation: u64,
}

#[derive(Debug)]
struct AdaptiveRuntimeState {
    last_menu_opened_at: Option<Timestamp>,
    last_coding_activity_at: Option<Timestamp>,
    is_low_power_mode: bool,
    thermal_pressure: ThermalPressure,
    scheduled_fire_at: Option<Timestamp>,
    fixed_refresh_interval: Option<Duration>,
    adaptive_computed_interval: Option<Duration>,
    sleep_override: Option<Duration>,
    timer_active: bool,
}

impl Default for AdaptiveRuntimeState {
    fn default() -> Self {
        Self {
            last_menu_opened_at: None,
            last_coding_activity_at: None,
            is_low_power_mode: false,
            thermal_pressure: ThermalPressure::Nominal,
            scheduled_fire_at: None,
            fixed_refresh_interval: None,
            adaptive_computed_interval: None,
            sleep_override: None,
            timer_active: false,
        }
    }
}

/// Core engine runtime coordinating provider refreshes, status feeds, Snapshot v1 assembly,
/// adaptive refresh cadence, edge-triggered transitions, and IPC v1 client intents.
///
/// # IPC Integration (`w-mac-3`)
///
/// The IPC server and daemon (`w-mac-3`) interact with [`EngineRuntime`] through:
/// - [`EngineRuntime::subscribe_snapshots`] (`tokio::sync::watch::Receiver<EngineSnapshot>`) to
///   push [`ServerMessage::Snapshot`] whenever a provider or batch refresh updates state.
/// - [`EngineRuntime::subscribe_settings_changes`] (`tokio::sync::broadcast::Receiver<SettingsScope>`)
///   to push [`ServerMessage::SettingsChanged`] when settings are mutated.
/// - [`EngineRuntime::get_snapshot`] for [`ClientMessage::GetSnapshot`].
/// - [`EngineRuntime::refresh_batch`], [`EngineRuntime::refresh_required`], and
///   [`EngineRuntime::refresh_provider`] for [`ClientMessage::Refresh`] and timer ticks.
/// - [`EngineRuntime::get_settings`] and [`EngineRuntime::set_setting`] for
///   [`ClientMessage::GetSettings`] and [`ClientMessage::SetSetting`].
/// - [`EngineRuntime::run_action`] for [`ClientMessage::RunAction`].
/// - [`EngineRuntime::handle_client_intent`] for a unified request/response dispatch helper.
pub struct EngineRuntime {
    config_store: ConfigStore,
    secret_store: Arc<SecretStore>,
    net: Arc<NetBroker>,
    cookie_cache: Arc<CookieCache>,
    cost_cache_root: RwLock<Option<std::path::PathBuf>>,
    status_fetcher: RwLock<ProviderStatusFetcher>,
    registry: RwLock<ProviderRegistry>,
    coordinator: Arc<ProviderRefreshCoordinator<ProviderId>>,
    snapshot_store: Arc<SnapshotStore>,
    sleeper: Arc<dyn Sleeper>,
    transition_sink: Arc<dyn TransitionSink>,
    notifier: Arc<dyn Notifier>,
    hook_runner: Option<Arc<HookRunner>>,
    clock: Arc<dyn Fn() -> Timestamp + Send + Sync>,
    base_environment: RwLock<BTreeMap<String, SecretString>>,
    hooks_config: RwLock<Option<HooksConfig>>,
    providers: Mutex<HashMap<ProviderId, ProviderRuntimeState>>,
    batch_state: Mutex<BatchRefreshState>,
    batch_idle_tx: watch::Sender<bool>,
    batch_idle_rx: watch::Receiver<bool>,
    required_completed_tx: watch::Sender<u64>,
    required_completed_rx: watch::Receiver<u64>,
    adaptive_state: Mutex<AdaptiveRuntimeState>,
    hook_detector: Mutex<HookTransitionDetector>,
    session_detector: Mutex<SessionQuotaTransitionDetector>,
    notification_policy: Mutex<NotificationPolicyEngine>,
    notification_state_path: RwLock<Option<std::path::PathBuf>>,
    settings_changed_tx: broadcast::Sender<SettingsScope>,
    pushes_tx: broadcast::Sender<ServerMessage>,
    next_request_seq: AtomicU64,
    completed_refresh_count: AtomicU64,
    weak_self: Mutex<Option<std::sync::Weak<Self>>>,
    timer_cancel: Mutex<Option<CancellationToken>>,
    timer_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    engine_version: String,
}

struct BatchRefreshGuard<'a> {
    runtime: &'a EngineRuntime,
}

impl Drop for BatchRefreshGuard<'_> {
    fn drop(&mut self) {
        {
            let mut batch = self.runtime.batch_state.lock().expect("batch_state lock");
            batch.is_refreshing = false;
            let _ = self.runtime.batch_idle_tx.send(true);
        }
        self.runtime.publish_current_snapshot();
    }
}

struct RequiredPassGuard<'a> {
    runtime: &'a EngineRuntime,
    pass_target: u64,
    completed_normally: bool,
}

impl Drop for RequiredPassGuard<'_> {
    fn drop(&mut self) {
        if self.completed_normally {
            return;
        }
        {
            let mut batch = self.runtime.batch_state.lock().expect("batch_state lock");
            batch.is_refreshing = false;
            let _ = self.runtime.batch_idle_tx.send(true);
            batch.required_completed_generation =
                batch.required_completed_generation.max(self.pass_target);
            let _ = self
                .runtime
                .required_completed_tx
                .send(batch.required_completed_generation);
            batch.is_required_loop_running = false;
        }
        self.runtime.publish_current_snapshot();
    }
}

impl Drop for EngineRuntime {
    fn drop(&mut self) {
        if let Some(cancel) = self.timer_cancel.lock().expect("timer_cancel lock").take() {
            cancel.cancel();
        }
        let _ = self.timer_task.lock().expect("timer_task lock").take();
    }
}

impl EngineRuntime {
    /// Creates a new `EngineRuntime` backed by `config_store`, `secret_store`, `net`, and `registry`.
    pub fn new(
        config_store: ConfigStore,
        secret_store: Arc<SecretStore>,
        net: Arc<NetBroker>,
        registry: ProviderRegistry,
    ) -> Self {
        let clock: Arc<dyn Fn() -> Timestamp + Send + Sync> = Arc::new(|| {
            let raw = Timestamp::now();
            Timestamp::from_second(raw.as_second()).unwrap_or(raw)
        });
        let now = (clock)();
        let config = config_store.load_effective().unwrap_or_else(|_| {
            ariadusage_core::config::normalize(Config::new(Config::CURRENT_VERSION, Vec::new()))
        });
        let initial_providers = Self::build_initial_provider_snapshots(&registry, &config);
        let initial_snapshot = EngineSnapshot::new(
            now,
            Self::stale_after_seconds(&config),
            EngineInfo {
                version: env!("CARGO_PKG_VERSION").to_string(),
                refreshing: false,
            },
            initial_providers,
        );

        let (batch_idle_tx, batch_idle_rx) = watch::channel(true);
        let (required_completed_tx, required_completed_rx) = watch::channel(0u64);
        let (settings_changed_tx, _) = broadcast::channel(32);
        let (pushes_tx, _) = broadcast::channel(64);

        let empty_env = BTreeMap::new();
        let default_inputs = ProviderPlanInputs::default();
        let mut provider_states = HashMap::new();
        for descriptor in registry.descriptors() {
            let provider_cfg = config.provider_config(descriptor.id.as_str());
            let selected = ProviderRegistry::selected_account(provider_cfg);
            let account_key = registry.compute_account_key(
                &descriptor.id,
                provider_cfg,
                selected.as_ref(),
                &empty_env,
                &default_inputs,
            );
            provider_states.insert(
                descriptor.id.clone(),
                ProviderRuntimeState {
                    account_key,
                    ..ProviderRuntimeState::default()
                },
            );
        }

        Self {
            config_store,
            secret_store,
            net,
            cookie_cache: Arc::new(CookieCache::new()),
            cost_cache_root: RwLock::new(None),
            status_fetcher: RwLock::new(ProviderStatusFetcher::default()),
            registry: RwLock::new(registry),
            coordinator: Arc::new(ProviderRefreshCoordinator::new()),
            snapshot_store: Arc::new(SnapshotStore::new(initial_snapshot)),
            sleeper: Arc::new(TokioSleeper),
            transition_sink: Arc::new(NoopTransitionSink),
            notifier: Arc::new(NoopNotifier),
            hook_runner: None,
            clock,
            base_environment: RwLock::new(BTreeMap::new()),
            hooks_config: RwLock::new(None),
            providers: Mutex::new(provider_states),
            batch_state: Mutex::new(BatchRefreshState::default()),
            batch_idle_tx,
            batch_idle_rx,
            required_completed_tx,
            required_completed_rx,
            adaptive_state: Mutex::new(AdaptiveRuntimeState::default()),
            hook_detector: Mutex::new(HookTransitionDetector::new()),
            session_detector: Mutex::new(SessionQuotaTransitionDetector::new()),
            notification_policy: Mutex::new(NotificationPolicyEngine::default()),
            notification_state_path: RwLock::new(None),
            settings_changed_tx,
            pushes_tx,
            next_request_seq: AtomicU64::new(1),
            completed_refresh_count: AtomicU64::new(0),
            weak_self: Mutex::new(None),
            timer_cancel: Mutex::new(None),
            timer_task: Mutex::new(None),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Attaches a shared [`CookieCache`] to this runtime.
    pub fn with_cookie_cache(mut self, cookie_cache: Arc<CookieCache>) -> Self {
        self.cookie_cache = cookie_cache;
        self
    }

    /// Returns a reference to the runtime's [`CookieCache`].
    pub fn cookie_cache(&self) -> &Arc<CookieCache> {
        &self.cookie_cache
    }

    /// Overrides the cache root directory (`<root>/cost-usage`) used when clearing cost usage caches (`clearCostCache`).
    pub fn with_cost_cache_root(self, root: impl Into<std::path::PathBuf>) -> Self {
        *self.cost_cache_root.write().expect("cost_cache_root lock") = Some(root.into());
        self
    }

    /// Replaces the [`ProviderStatusFetcher`] (for example, to configure local test server URLs).
    pub fn with_status_fetcher(self, status_fetcher: ProviderStatusFetcher) -> Self {
        *self.status_fetcher.write().expect("status_fetcher lock") = status_fetcher;
        self
    }

    /// Replaces the [`Sleeper`] used for delayed retries.
    pub fn with_sleeper(mut self, sleeper: Arc<dyn Sleeper>) -> Self {
        self.sleeper = sleeper;
        self
    }

    /// Attaches a [`TransitionSink`] to receive edge-triggered hook dispatches, session quota
    /// transitions, and notification posts.
    pub fn with_transition_sink(mut self, sink: Arc<dyn TransitionSink>) -> Self {
        self.transition_sink = sink;
        self
    }

    /// Attaches a [`Notifier`] backend to deliver emitted [`NotificationPost`]s.
    pub fn with_notifier(mut self, notifier: Arc<dyn Notifier>) -> Self {
        self.notifier = notifier;
        self
    }

    /// Configures the file path (`$XDG_STATE_HOME/ariadusage/notifications-state.json`, mode `0600`)
    /// used to load and persist edge-triggered notification state across engine restarts.
    pub fn with_notification_state_path(self, path: std::path::PathBuf) -> Self {
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(persisted) = serde_json::from_slice::<
                ariadusage_core::notifications::NotificationPersistedState,
            >(&bytes)
        {
            self.notification_policy
                .lock()
                .expect("notification_policy lock")
                .restore_persisted_state(persisted);
        }
        *self
            .notification_state_path
            .write()
            .expect("notification_state_path lock") = Some(path);
        self
    }

    /// Attaches a [`HookRunner`] to execute matching hook rules when transitions occur.
    pub fn with_hook_runner(mut self, hook_runner: Arc<HookRunner>) -> Self {
        self.hook_runner = Some(hook_runner);
        self
    }

    /// Returns a clone of the broadcast sender used for [`ServerMessage`] IPC pushes.
    pub fn pushes_sender(&self) -> broadcast::Sender<ServerMessage> {
        self.pushes_tx.clone()
    }

    /// Replaces the wall-clock provider used for snapshot timestamps and pace calculation.
    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> Timestamp + Send + Sync>) -> Self {
        self.clock = clock;
        self.publish_current_snapshot();
        self
    }

    /// Sets the base environment map passed into provider plan resolution and updates initial
    /// provider account keys accordingly.
    pub fn with_base_environment(self, env: BTreeMap<String, SecretString>) -> Self {
        *self.base_environment.write().expect("base_env lock") = env.clone();
        let config = self.load_config();
        let registry = self.registry.read().expect("registry lock").clone();
        let inputs = ProviderPlanInputs {
            base_environment: env,
            ..ProviderPlanInputs::default()
        };
        let mut states = self.providers.lock().expect("providers lock");
        for descriptor in registry.descriptors() {
            let provider_cfg = config.provider_config(descriptor.id.as_str());
            let selected = ProviderRegistry::selected_account(provider_cfg);
            let projected_env = ProviderRegistry::resolve_environment(
                descriptor,
                provider_cfg,
                selected.as_ref(),
                &inputs,
            );
            let account_key = registry.compute_account_key(
                &descriptor.id,
                provider_cfg,
                selected.as_ref(),
                &projected_env,
                &inputs,
            );
            if let Some(state) = states.get_mut(&descriptor.id) {
                state.account_key = account_key;
            }
        }
        drop(states);
        self
    }

    /// Seeds a cached [`UsageSnapshot`] and reset-backfill snapshot for `id` (mirrors upstream
    /// `_setSnapshotForTesting`).
    pub fn seed_provider_snapshot(&self, id: ProviderId, snapshot: UsageSnapshot) {
        {
            let mut states = self.providers.lock().expect("providers lock");
            let state = states.entry(id.clone()).or_default();
            let scoped = snapshot.scoped_to(id);
            state.snapshot = Some(scoped.clone());
            state.last_known_reset_snapshot = Some(scoped);
            state.last_error = None;
            state.refresh_state = RefreshState::Fresh;
            state.kept_snapshot = false;
        }
        self.publish_current_snapshot();
    }

    /// Sets the [`HooksConfig`] evaluated by the edge-triggered [`HookTransitionDetector`].
    pub fn with_hooks_config(self, hooks_config: HooksConfig) -> Self {
        *self.hooks_config.write().expect("hooks_config lock") = Some(hooks_config);
        self
    }

    /// Updates the [`ProviderStatusFetcher`] in place.
    pub fn set_status_fetcher(&self, status_fetcher: ProviderStatusFetcher) {
        *self.status_fetcher.write().expect("status_fetcher lock") = status_fetcher;
    }

    /// Registers or replaces a [`ariadusage_core::pipeline::ProviderPlan`] for `id` in the registry.
    pub fn register_provider_plan(
        &self,
        id: ProviderId,
        plan: Arc<dyn ariadusage_core::pipeline::ProviderPlan>,
    ) {
        self.registry
            .write()
            .expect("registry lock")
            .register_plan(id, plan);
    }

    /// Returns a reference to the underlying [`SnapshotStore`].
    pub fn snapshot_store(&self) -> &Arc<SnapshotStore> {
        &self.snapshot_store
    }

    /// Subscribes to [`EngineSnapshot`] updates via [`tokio::sync::watch::Receiver`].
    pub fn subscribe_snapshots(&self) -> watch::Receiver<EngineSnapshot> {
        self.snapshot_store.subscribe()
    }

    /// Subscribes to [`SettingsScope`] change notifications for IPC `settingsChanged` pushes.
    pub fn subscribe_settings_changes(&self) -> broadcast::Receiver<SettingsScope> {
        self.settings_changed_tx.subscribe()
    }

    /// Returns the latest assembled [`EngineSnapshot`].
    pub fn current_snapshot(&self) -> EngineSnapshot {
        self.snapshot_store.current()
    }

    /// Returns a reference to the per-provider [`ProviderRefreshCoordinator`].
    pub fn coordinator(&self) -> &Arc<ProviderRefreshCoordinator<ProviderId>> {
        &self.coordinator
    }

    fn now(&self) -> Timestamp {
        (self.clock)()
    }

    fn load_config(&self) -> Config {
        self.config_store.load_effective().unwrap_or_else(|_| {
            ariadusage_core::config::normalize(Config::new(Config::CURRENT_VERSION, Vec::new()))
        })
    }

    fn stale_after_seconds(config: &Config) -> u32 {
        let prefs = AppPreferences::from_config(config);
        match prefs.refresh_frequency.duration() {
            Some(d) => (d.as_secs() as u32).max(60),
            None => DEFAULT_STALE_AFTER_SECONDS,
        }
    }

    fn build_initial_provider_snapshots(
        registry: &ProviderRegistry,
        config: &Config,
    ) -> Vec<ProviderSnapshot> {
        registry
            .ordered_descriptors(config)
            .into_iter()
            .map(|descriptor| {
                let enabled = registry.is_provider_enabled(config, &descriptor.id);
                let provider_cfg = config.provider_config(descriptor.id.as_str());
                let selected = ProviderRegistry::selected_account(provider_cfg);
                let mode = ProviderRegistry::effective_source_mode(
                    descriptor,
                    provider_cfg,
                    selected.as_ref(),
                    None,
                );
                let mut presentation = descriptor.presentation();
                if let Some(custom_color) = provider_cfg.and_then(|c| c.accent_color.clone()) {
                    presentation.accent_color = Some(custom_color);
                }
                let mut snap = ProviderSnapshot::new(
                    descriptor.id.clone(),
                    descriptor.display_name,
                    enabled,
                    source_mode_label(mode),
                );
                snap.presentation = Some(presentation);
                snap
            })
            .collect()
    }

    /// Assembles and publishes the current [`EngineSnapshot`] into [`SnapshotStore`].
    pub fn publish_current_snapshot(&self) -> EngineSnapshot {
        let config = self.load_config();
        let now = self.now();
        let refreshing = self
            .batch_state
            .lock()
            .expect("batch_state lock")
            .is_refreshing;
        let registry = self.registry.read().expect("registry lock").clone();
        let ordered_descriptors = registry.ordered_descriptors(&config);
        let states = self.providers.lock().expect("providers lock");

        let mut provider_snapshots = Vec::with_capacity(ordered_descriptors.len());
        for descriptor in ordered_descriptors {
            let enabled = registry.is_provider_enabled(&config, &descriptor.id);
            let state = states.get(&descriptor.id).cloned().unwrap_or_default();
            let provider_cfg = config.provider_config(descriptor.id.as_str());
            let selected = ProviderRegistry::selected_account(provider_cfg);
            let mode = ProviderRegistry::effective_source_mode(
                descriptor,
                provider_cfg,
                selected.as_ref(),
                None,
            );
            let mut snap = Self::project_provider_state(
                descriptor,
                &state,
                enabled,
                source_mode_label(mode),
                now,
            );
            if let Some(custom_color) = provider_cfg.and_then(|c| c.accent_color.clone())
                && let Some(pres) = snap.presentation.as_mut()
            {
                pres.accent_color = Some(custom_color);
            }
            provider_snapshots.push(snap);
        }
        drop(states);

        let engine_snapshot = EngineSnapshot::new(
            now,
            Self::stale_after_seconds(&config),
            EngineInfo {
                version: self.engine_version.clone(),
                refreshing,
            },
            provider_snapshots,
        );
        self.snapshot_store.publish(engine_snapshot.clone());
        let _ = self.pushes_tx.send(ServerMessage::Snapshot {
            snapshot: Box::new(engine_snapshot.clone()),
        });
        engine_snapshot
    }

    fn project_provider_state(
        descriptor: &ProviderDescriptor,
        state: &ProviderRuntimeState,
        enabled: bool,
        source_mode: &str,
        now: Timestamp,
    ) -> ProviderSnapshot {
        let mut projected = if let Some(usage) = &state.snapshot {
            let pace = usage.secondary.as_ref().and_then(|secondary| {
                let is_weekly = secondary.window_minutes.is_none()
                    || secondary.window_minutes
                        == Some(ariadusage_core::pace::DEFAULT_WEEKLY_WINDOW_MINUTES);
                if !is_weekly
                    && !descriptor
                        .pace_capability
                        .supports_reset_window_pace(secondary, now)
                {
                    return None;
                }
                let resolved = descriptor
                    .pace_capability
                    .resolved_reset_window_for_pace(secondary);
                UsagePace::weekly(
                    &resolved,
                    now,
                    resolved.window_minutes,
                    None,
                    &TimeZone::system(),
                )
            });

            let input = ProjectionInput {
                provider: descriptor.id.clone(),
                display_name: descriptor.display_name.to_string(),
                enabled,
                source_mode: source_mode.to_string(),
                refresh: state.refresh_state,
                kept_snapshot: state.kept_snapshot,
                as_of: usage.updated_at,
                source_kind: state.source_kind,
                source_label: state.source_label.clone(),
                error: state.last_error.clone(),
                pace,
            };
            provider_snapshot(usage, &input)
        } else {
            let mut empty = ProviderSnapshot::new(
                descriptor.id.clone(),
                descriptor.display_name,
                enabled,
                source_mode,
            );
            empty.presentation = Some(descriptor.presentation());
            empty.last_error =
                state
                    .last_error
                    .as_ref()
                    .map(|e| ariadusage_protocol::ProviderError {
                        kind: e.kind,
                        category: e.category,
                        message: e.category.safe_description().to_string(),
                        retry_after_seconds: e.retry_after_seconds,
                    });
            empty
        };

        if let Some(status) = &state.status {
            let status_state = if state.refresh_state == RefreshState::Error && state.kept_snapshot
            {
                MetricState::Stale
            } else {
                MetricState::Value
            };
            let as_of = state.status_as_of.or(status.updated_at).or(Some(now));
            if let Ok(metric) = Metric::new(
                status_state,
                Some(status.clone()),
                as_of,
                Some(MetricSource::new(SourceKind::Web, "status")),
                Confidence::Exact,
                None,
            ) {
                projected.status = Some(metric);
            }
        }

        projected
    }

    /// Runs an opportunistic batch refresh across all enabled providers.
    ///
    /// Enforces `ARCHITECTURE.md` §6.4 ("One refresh batch at a time"): if a batch refresh is
    /// already in flight, returns `false` immediately without starting a second batch.
    pub async fn refresh_batch(&self, options: RefreshOptions) -> bool {
        {
            let mut batch = self.batch_state.lock().expect("batch_state lock");
            if batch.is_refreshing {
                return false;
            }
            batch.is_refreshing = true;
            let _ = self.batch_idle_tx.send(false);
        }

        let _guard = BatchRefreshGuard { runtime: self };
        self.publish_current_snapshot();
        self.execute_batch_pass(options).await;
        true
    }

    /// Runs a required batch refresh (for example on user manual refresh or settings mutation),
    /// coalescing concurrent callers into at most one queued follow-up pass after the current
    /// batch finishes.
    ///
    /// Ported from `UsageStore+Refresh.swift` (`performRequiredRefresh`).
    pub async fn refresh_required(&self, options: RefreshOptions) {
        let (requested_gen, should_run_loop) = {
            let mut batch = self.batch_state.lock().expect("batch_state lock");
            batch.required_request_generation = batch.required_request_generation.saturating_add(1);
            let req_gen = batch.required_request_generation;
            if batch.is_required_loop_running {
                (req_gen, false)
            } else {
                batch.is_required_loop_running = true;
                (req_gen, true)
            }
        };

        if !should_run_loop {
            let mut rx = self.required_completed_rx.clone();
            while *rx.borrow() < requested_gen {
                if rx.changed().await.is_err() {
                    break;
                }
            }
            return;
        }

        loop {
            let pass_target = {
                let batch = self.batch_state.lock().expect("batch_state lock");
                if batch.required_completed_generation >= batch.required_request_generation {
                    break;
                }
                batch.required_request_generation
            };

            // Enforce §6.4 ("One refresh batch at a time"): always wait for any running batch
            // to complete before starting the required pass.
            let mut idle_rx = self.batch_idle_rx.clone();
            while !*idle_rx.borrow() {
                if idle_rx.changed().await.is_err() {
                    break;
                }
            }

            {
                let mut batch = self.batch_state.lock().expect("batch_state lock");
                batch.is_refreshing = true;
                let _ = self.batch_idle_tx.send(false);
            }
            let mut pass_guard = RequiredPassGuard {
                runtime: self,
                pass_target,
                completed_normally: false,
            };
            self.publish_current_snapshot();

            self.execute_batch_pass(options).await;

            pass_guard.completed_normally = true;
            let done = {
                let mut batch = self.batch_state.lock().expect("batch_state lock");
                batch.is_refreshing = false;
                let _ = self.batch_idle_tx.send(true);
                batch.required_completed_generation = pass_target;
                let _ = self.required_completed_tx.send(pass_target);
                if batch.required_completed_generation >= batch.required_request_generation {
                    batch.is_required_loop_running = false;
                    true
                } else {
                    false
                }
            };
            self.publish_current_snapshot();

            if done {
                break;
            }
        }
    }

    async fn execute_batch_pass(&self, options: RefreshOptions) {
        let config = self.load_config();
        let prefs = AppPreferences::from_config(&config);
        let enabled_ids: Vec<ProviderId> = self
            .registry
            .read()
            .expect("registry lock")
            .enabled_descriptors(&config)
            .into_iter()
            .map(|d| d.id.clone())
            .collect();

        let mut provider_options = options;
        provider_options.refresh_status = options.refresh_status && prefs.status_checks_enabled;

        let futures: Vec<BoxFuture<'_, ()>> = enabled_ids
            .into_iter()
            .map(|id| Box::pin(self.refresh_provider(id, provider_options)) as BoxFuture<'_, ()>)
            .collect();
        join_all_futures(futures).await;
        self.completed_refresh_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Refreshes a single provider while enforcing per-provider coalescing, replacement ordering,
    /// stale-generation suppression, cancellation neutrality, consecutive failure gating,
    /// non-preservable error dropping, account-change resets, and the identity silo.
    pub async fn refresh_provider(&self, id: ProviderId, options: RefreshOptions) {
        loop {
            if options.coalesce_if_refreshing
                && let Some(existing) = self.coordinator.coalescing_state(&id)
            {
                let wait_res = self.coordinator.wait(&id, &existing).await;
                if wait_res == WaitResult::RetryRequired {
                    continue;
                }
                return;
            }

            let replacement = self.coordinator.begin_replacing_request(id.clone());
            let request_state = replacement.state;
            let generation = replacement.generation;

            ProviderRefreshCoordinator::<ProviderId>::wait_predecessors(
                &replacement.predecessor_states,
            )
            .await;

            if request_state.is_cancelled() || !self.coordinator.is_current(generation, &id) {
                self.coordinator.complete(&request_state, &id, true);
                return;
            }

            let config = self.load_config();
            let prefs = AppPreferences::from_config(&config);
            let inputs = self
                .resolve_plan_inputs(&id, &config, &prefs, options.interaction, &request_state)
                .await;

            let Some(spec) = self
                .registry
                .read()
                .expect("registry lock")
                .resolve_spec(&id, &config, &inputs)
            else {
                self.coordinator.complete(&request_state, &id, false);
                return;
            };

            if !spec.enabled {
                self.clear_disabled_provider(&id);
                self.coordinator.complete(&request_state, &id, false);
                self.publish_current_snapshot();
                return;
            }

            let expected_revision = self
                .reconcile_account_key(&id, spec.account_key.clone())
                .await;

            self.coordinator.begin_activity(id.clone());

            let req_seq = self.next_request_seq.fetch_add(1, Ordering::Relaxed);
            let cx = FetchContext {
                provider: id.clone(),
                runtime: FetchRuntime::Daemon,
                interaction: options.interaction,
                phase: options.phase,
                request_id: format!("refresh-{}-{req_seq}", id.as_str()),
                source_mode: spec.source_mode,
                env: spec.environment.clone(),
                include_credits: prefs.show_optional_credits_and_extra_usage,
                selected_token_account: spec.selected_account.as_ref().map(|a| a.id.clone()),
                cancel: request_state.cancel_token(),
            };

            let outcome = fetch_outcome(&id, spec.plan.as_ref(), &cx, self.sleeper.as_ref()).await;

            self.coordinator.end_activity(&id);

            let post_account_key = self
                .registry
                .read()
                .expect("registry lock")
                .compute_account_key(
                    &id,
                    config.provider_config(id.as_str()),
                    spec.selected_account.as_ref(),
                    &spec.environment,
                    &inputs,
                );
            let expected_revision = if post_account_key != spec.account_key {
                self.reconcile_account_key(&id, post_account_key).await
            } else {
                expected_revision
            };

            let retry_required = self
                .apply_fetch_outcome(
                    &id,
                    &spec,
                    outcome,
                    generation,
                    expected_revision,
                    &request_state,
                    options.surface_policy,
                )
                .await;

            self.coordinator
                .complete(&request_state, &id, retry_required);

            if !retry_required
                && options.refresh_status
                && prefs.status_checks_enabled
                && spec.descriptor.supports_status_polling()
            {
                let _ = self.refresh_provider_status(id.clone()).await;
            }

            self.publish_current_snapshot();
            return;
        }
    }

    async fn resolve_plan_inputs(
        &self,
        id: &ProviderId,
        config: &Config,
        prefs: &AppPreferences,
        interaction: FetchInteraction,
        request_state: &Arc<crate::coordinator::ProviderRefreshRequestState>,
    ) -> ProviderPlanInputs {
        let base_environment = self.base_environment.read().expect("base_env lock").clone();
        Self::resolve_plan_inputs_with_store(
            Some(&self.secret_store),
            id,
            config,
            prefs,
            base_environment,
            interaction,
            request_state.cancel_token(),
            format!("secret-{}-{}", id.as_str(), request_state.id()),
        )
        .await
    }

    /// Resolves [`ProviderPlanInputs`] from an optional [`SecretStore`], `config`, `prefs`, and
    /// `base_environment` for a single provider fetch.
    #[allow(clippy::too_many_arguments)]
    pub async fn resolve_plan_inputs_with_store(
        secret_store: Option<&Arc<SecretStore>>,
        id: &ProviderId,
        config: &Config,
        prefs: &AppPreferences,
        base_environment: BTreeMap<String, SecretString>,
        interaction: FetchInteraction,
        cancel: CancellationToken,
        request_id: String,
    ) -> ProviderPlanInputs {
        let call = BrokerCall {
            interaction,
            cancel,
            request_id,
        };

        let api_key = if let Some(store) = secret_store {
            Self::lookup_secret_in_store(
                store,
                &format!("providers.{}.apiKey", id.as_str()),
                interaction,
                &call,
            )
            .await
        } else {
            None
        };
        let manual_cookie_header = if let Some(store) = secret_store {
            Self::lookup_secret_in_store(
                store,
                &format!("providers.{}.cookieHeader", id.as_str()),
                interaction,
                &call,
            )
            .await
        } else {
            None
        };

        let provider_cfg = config.provider_config(id.as_str());
        let selected_account = ProviderRegistry::selected_account(provider_cfg);
        let selected_account_token = match (secret_store, &selected_account) {
            (Some(store), Some(acct)) => {
                Self::lookup_secret_in_store(
                    store,
                    &format!("providers.{}.accounts.{}.token", id.as_str(), acct.id),
                    interaction,
                    &call,
                )
                .await
            }
            _ => None,
        };

        ProviderPlanInputs {
            base_environment,
            api_key,
            manual_cookie_header,
            selected_account_token,
            include_optional_usage: prefs.show_optional_credits_and_extra_usage,
            secret_store: secret_store.cloned(),
        }
    }

    /// Looks up a single secret by setting ID string in `secret_store` under `interaction`.
    pub async fn lookup_secret_in_store(
        secret_store: &SecretStore,
        setting_id_str: &str,
        interaction: FetchInteraction,
        call: &BrokerCall,
    ) -> Option<SecretString> {
        let setting_id = SettingId::new(setting_id_str).ok()?;
        let secret_id = SecretId::from_setting_id(&setting_id).ok()?;
        let lookup = match interaction {
            FetchInteraction::Background => {
                secret_store
                    .background(call.clone())
                    .lookup(&secret_id)
                    .await
            }
            FetchInteraction::UserInitiated => {
                secret_store.user(call.clone()).lookup(&secret_id).await
            }
        };
        match lookup {
            SecretLookup::Found(secret) => Some(secret),
            _ => None,
        }
    }

    fn clear_disabled_provider(&self, id: &ProviderId) {
        self.coordinator.invalidate_requests(id);
        let mut states = self.providers.lock().expect("providers lock");
        let state = states.entry(id.clone()).or_default();
        state.enablement_revision = state.enablement_revision.saturating_add(1);
        state.snapshot = None;
        state.last_known_reset_snapshot = None;
        state.last_error = None;
        state.failure_gate.reset();
        state.refresh_state = RefreshState::Fresh;
        state.kept_snapshot = false;
    }

    async fn reconcile_account_key(&self, id: &ProviderId, new_account_key: Option<String>) -> u64 {
        let changed = {
            let mut states = self.providers.lock().expect("providers lock");
            let state = states.entry(id.clone()).or_default();
            if state.account_key != new_account_key {
                let decision = on_account_change(&mut state.failure_gate);
                if decision.drop_snapshot {
                    state.snapshot = None;
                    state.last_known_reset_snapshot = None;
                }
                state.last_error = None;
                state.refresh_state = RefreshState::Fresh;
                state.kept_snapshot = false;
                state.account_key = new_account_key;
                state.enablement_revision = state.enablement_revision.saturating_add(1);
                Some(state.enablement_revision)
            } else {
                None
            }
        };

        if let Some(rev) = changed {
            self.session_detector
                .lock()
                .expect("session_detector lock")
                .reset(id.as_str());
            self.hook_detector
                .lock()
                .expect("hook_detector lock")
                .reset_provider(id.as_str());
            self.notification_policy
                .lock()
                .expect("notification_policy lock")
                .reset_provider_for_account_switch(id);
            rev
        } else {
            self.providers
                .lock()
                .expect("providers lock")
                .get(id)
                .map_or(0, |s| s.enablement_revision)
        }
    }

    /// Explicitly switches the active account key for `id`, enforcing `ARCHITECTURE.md` §6.4:
    /// drops prior snapshot and reset-time cache, resets `FailureGate`, clears `last_error`,
    /// invalidates in-flight requests, resets edge-triggered transition baselines, and publishes
    /// the updated [`EngineSnapshot`].
    pub async fn switch_account(&self, id: &ProviderId, new_account_key: Option<String>) {
        self.coordinator.invalidate_requests(id);
        {
            let mut states = self.providers.lock().expect("providers lock");
            let state = states.entry(id.clone()).or_default();
            let decision = on_account_change(&mut state.failure_gate);
            if decision.drop_snapshot {
                state.snapshot = None;
                state.last_known_reset_snapshot = None;
            }
            state.last_error = None;
            state.refresh_state = RefreshState::Fresh;
            state.kept_snapshot = false;
            state.account_key = new_account_key;
            state.enablement_revision = state.enablement_revision.saturating_add(1);
        }
        self.session_detector
            .lock()
            .expect("session_detector lock")
            .reset(id.as_str());
        self.hook_detector
            .lock()
            .expect("hook_detector lock")
            .reset_provider(id.as_str());
        self.notification_policy
            .lock()
            .expect("notification_policy lock")
            .reset_provider_for_account_switch(id);
        self.publish_current_snapshot();
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_fetch_outcome(
        &self,
        id: &ProviderId,
        spec: &ResolvedProviderSpec,
        outcome: ariadusage_core::pipeline::FetchOutcome,
        generation: u64,
        expected_revision: u64,
        request_state: &Arc<crate::coordinator::ProviderRefreshRequestState>,
        surface_policy: SurfacePolicy,
    ) -> bool {
        if request_state.is_cancelled() || !self.coordinator.is_current(generation, id) {
            return true;
        }

        let now = self.now();
        match outcome.result {
            Ok(fetch_result) => {
                let mut usage = fetch_result.usage.scoped_to(id.clone());
                if let Some(acct) = &spec.selected_account {
                    usage = usage.with_account_label(&acct.label, id);
                }

                let mut identity_changed = false;
                let should_eval_transitions = {
                    let mut states = self.providers.lock().expect("providers lock");
                    let state = states.entry(id.clone()).or_default();
                    if !self.coordinator.is_current(generation, id)
                        || state.enablement_revision != expected_revision
                        || request_state.is_cancelled()
                    {
                        return true;
                    }

                    // Identity silo & account change detection from returned snapshot identity
                    if let Some(prev) = &state.snapshot
                        && prev.identity.is_some()
                        && usage.identity.is_some()
                        && !identities_match(usage.identity.as_ref(), prev.identity.as_ref())
                    {
                        let _ = on_account_change(&mut state.failure_gate);
                        state.snapshot = None;
                        state.last_known_reset_snapshot = None;
                        identity_changed = true;
                    }

                    let backfilled = usage
                        .backfilling_reset_times(state.last_known_reset_snapshot.as_ref(), now);
                    state.snapshot = Some(backfilled.clone());
                    state.last_known_reset_snapshot = Some(backfilled);
                    state.failure_gate.record_success();
                    state.last_error = None;
                    state.refresh_state = RefreshState::Fresh;
                    state.kept_snapshot = false;
                    state.source_kind = fetch_result.strategy_kind;
                    state.source_label = fetch_result.source_label;
                    true
                };

                if identity_changed {
                    self.session_detector
                        .lock()
                        .expect("session_detector lock")
                        .reset(id.as_str());
                    self.hook_detector
                        .lock()
                        .expect("hook_detector lock")
                        .reset_provider(id.as_str());
                    self.notification_policy
                        .lock()
                        .expect("notification_policy lock")
                        .reset_provider_for_account_switch(id);
                }

                if should_eval_transitions {
                    self.evaluate_transitions_with(id, true, None).await;
                }
                false
            }
            Err(err) => {
                // Invariant 4: Cancellation is not a failure.
                if is_cancellation(&err) || request_state.is_cancelled() {
                    return true;
                }

                let preservable = is_preservable_error(&err);
                let projected_error = map_fetch_error(&err);

                let should_eval_transitions = {
                    let mut states = self.providers.lock().expect("providers lock");
                    let state = states.entry(id.clone()).or_default();
                    if !self.coordinator.is_current(generation, id)
                        || state.enablement_revision != expected_revision
                        || request_state.is_cancelled()
                    {
                        return true;
                    }

                    let had_prior = state.snapshot.is_some();
                    let decision = failure_decision(
                        FailureInput::new(had_prior, preservable, surface_policy, false),
                        &mut state.failure_gate,
                    );

                    if decision.drop_snapshot {
                        state.snapshot = None;
                    }

                    let still_has_prior = state.snapshot.is_some();
                    let (refresh_state, kept_snapshot) =
                        decision.to_projection_refresh(had_prior && !decision.drop_snapshot);
                    state.refresh_state = refresh_state;
                    state.kept_snapshot = kept_snapshot && still_has_prior;

                    if decision.surface_error {
                        state.last_error = Some(projected_error);
                    } else {
                        state.last_error = None;
                    }

                    decision.surface_error
                };

                if should_eval_transitions {
                    self.evaluate_transitions_with(id, false, Some(&err)).await;
                }
                false
            }
        }
    }

    /// Polls the status feed for `id` through `NetBroker` with declared origins and applies
    /// monotonic generation publication rules (`UsageStore+ProviderStatus.swift`).
    ///
    /// - Only a successful status fetch advances `status_published_generation`.
    /// - A failed newer status fetch does not retire an older in-flight status fetch that later
    ///   succeeds, and preserves any previously published status and components.
    pub async fn refresh_provider_status(
        &self,
        id: ProviderId,
    ) -> Result<Option<ProviderStatus>, StatusFetchError> {
        let Some(descriptor) = self.registry.read().expect("registry lock").descriptor(&id) else {
            return Ok(None);
        };
        if !descriptor.supports_status_polling() {
            return Ok(None);
        }

        let (request_generation, expected_revision) = {
            let mut states = self.providers.lock().expect("providers lock");
            let state = states.entry(id.clone()).or_default();
            state.status_request_generation = state.status_request_generation.saturating_add(1);
            (state.status_request_generation, state.enablement_revision)
        };

        let fetcher = self
            .status_fetcher
            .read()
            .expect("status_fetcher lock")
            .clone();
        let call = BrokerCall {
            interaction: FetchInteraction::Background,
            cancel: CancellationToken::new(),
            request_id: format!("status-{}-{request_generation}", id.as_str()),
        };

        let fetch_res = fetcher
            .fetch_for_descriptor(&self.net, &call, descriptor)
            .await;

        match fetch_res {
            Ok(Some(summary)) => {
                let now = self.now();
                let mut status = summary.status;
                if status.page_url.is_none()
                    && let Some(link) = descriptor.effective_status_link_url()
                {
                    status.page_url = Some(link.to_string());
                }

                let published = {
                    let mut states = self.providers.lock().expect("providers lock");
                    let state = states.entry(id.clone()).or_default();
                    if state.enablement_revision != expected_revision
                        || request_generation <= state.status_published_generation
                    {
                        false
                    } else {
                        state.status_published_generation = request_generation;
                        state.status = Some(status.clone());
                        state.status_components = summary.components;
                        state.status_as_of = Some(now);
                        true
                    }
                };

                if published {
                    self.evaluate_transitions_with(&id, false, None).await;
                    self.publish_current_snapshot();
                    Ok(Some(status))
                } else {
                    let existing = self
                        .providers
                        .lock()
                        .expect("providers lock")
                        .get(&id)
                        .and_then(|s| s.status.clone());
                    Ok(existing)
                }
            }
            Ok(None) => Ok(None),
            Err(err) => {
                // Failed status fetches preserve last-good status and do not advance
                // status_published_generation so an older in-flight fetch can still publish.
                Err(err)
            }
        }
    }

    /// Returns the latest cached Statuspage component breakdown for `id`.
    pub fn provider_status_components(&self, id: &ProviderId) -> Vec<StatuspageComponentSummary> {
        self.providers
            .lock()
            .expect("providers lock")
            .get(id)
            .map(|s| s.status_components.clone())
            .unwrap_or_default()
    }

    async fn evaluate_transitions_with(
        &self,
        id: &ProviderId,
        is_usage_refresh_success: bool,
        fetch_err: Option<&FetchError>,
    ) {
        let now = self.now();
        let config = self.load_config();
        let prefs = AppPreferences::from_config(&config);
        let provider_cfg = config.provider_config(id.as_str()).cloned();
        let warnings = provider_cfg.as_ref().and_then(|c| c.quota_warnings.clone());

        let (observation, session_remaining, snapshot_opt, account_disc_for_policy, source_label) = {
            let states = self.providers.lock().expect("providers lock");
            let Some(state) = states.get(id) else {
                return;
            };
            let session_rem = state
                .snapshot
                .as_ref()
                .and_then(|s| s.primary.as_ref())
                .filter(|w| !w.is_synthetic_placeholder)
                .map(|w| w.remaining_percent());

            let mut lanes = Vec::new();
            let mut secondary_window = None;
            let mut account_display = None;
            let mut account_discriminator = state.account_key.clone();

            if let Some(snap) = &state.snapshot {
                account_display = hook_account_display_name(id, snap, prefs.hide_personal_info);
                if let Some(disc) = hook_account_discriminator(id, snap) {
                    account_discriminator = Some(disc);
                }

                let session_thresholds = warnings
                    .as_ref()
                    .map(|w| {
                        w.thresholds(QuotaWarningWindow::Session, &prefs.quota_warning_thresholds)
                    })
                    .unwrap_or_else(|| prefs.quota_warning_thresholds.clone());
                let session_fallback: Vec<f64> = session_thresholds
                    .into_iter()
                    .map(|t| (100.0 - f64::from(t)) / 100.0)
                    .collect();

                let weekly_thresholds = warnings
                    .as_ref()
                    .map(|w| {
                        w.thresholds(QuotaWarningWindow::Weekly, &prefs.quota_warning_thresholds)
                    })
                    .unwrap_or_else(|| prefs.quota_warning_thresholds.clone());
                let weekly_fallback: Vec<f64> = weekly_thresholds
                    .into_iter()
                    .map(|t| (100.0 - f64::from(t)) / 100.0)
                    .collect();

                lanes.push(
                    HookQuotaLaneObservation::new(
                        HookQuotaLaneKey::new(
                            id.as_str(),
                            QuotaWarningWindow::Session,
                            account_discriminator.clone(),
                            None,
                        ),
                        "Session",
                        snap.primary.clone(),
                        session_fallback,
                    )
                    .with_account_display_name(account_display.clone()),
                );
                lanes.push(
                    HookQuotaLaneObservation::new(
                        HookQuotaLaneKey::new(
                            id.as_str(),
                            QuotaWarningWindow::Weekly,
                            account_discriminator.clone(),
                            None,
                        ),
                        "Weekly",
                        snap.secondary.clone(),
                        weekly_fallback.clone(),
                    )
                    .with_account_display_name(account_display.clone()),
                );
                if let Some(extra_windows) = &snap.extra_rate_windows {
                    for named in extra_windows {
                        lanes.push(
                            HookQuotaLaneObservation::new(
                                HookQuotaLaneKey::new(
                                    id.as_str(),
                                    QuotaWarningWindow::Weekly,
                                    account_discriminator.clone(),
                                    Some(named.id.clone()),
                                ),
                                named.title.clone(),
                                Some(named.window.clone()),
                                weekly_fallback.clone(),
                            )
                            .with_account_display_name(account_display.clone()),
                        );
                    }
                }
                secondary_window = snap.secondary.clone();
            }

            let status = state
                .status
                .as_ref()
                .map_or(HookProviderStatus::Unknown, |st| {
                    HookProviderStatus::from(st.indicator)
                });
            let refresh_failure_status = state
                .last_error
                .as_ref()
                .map(|e| e.category.safe_description().to_string());

            let account_disc_for_policy = account_discriminator.clone();
            let obs = HookProviderObservation::new(
                id.as_str(),
                account_display,
                lanes,
                secondary_window,
                status,
                refresh_failure_status,
            )
            .with_successful_usage(if is_usage_refresh_success {
                state.snapshot.clone()
            } else {
                None
            })
            .with_account_discriminator(account_discriminator);

            (
                obs,
                session_rem,
                state.snapshot.clone(),
                account_disc_for_policy,
                state.source_label.clone(),
            )
        };

        let transition = self
            .session_detector
            .lock()
            .expect("session_detector lock")
            .evaluate(id.as_str(), session_remaining);
        if transition != SessionQuotaTransition::None {
            self.transition_sink
                .on_session_quota_transition(id, transition);
        }

        let hooks_cfg = self
            .hooks_config
            .read()
            .expect("hooks_config lock")
            .clone()
            .or_else(|| config.hooks_config())
            .unwrap_or_default();

        // Evaluate NotificationPolicyEngine
        let mut emitted_posts = Vec::new();
        {
            let mut policy = self
                .notification_policy
                .lock()
                .expect("notification_policy lock");
            let mut notif_prefs = prefs.notification_preferences();
            if let Some(qw) = &warnings {
                notif_prefs
                    .provider_quota_warnings
                    .insert(id.clone(), qw.clone());
            }
            let cred_enabled = notif_prefs.credential_expiry_notifications_enabled;
            policy.preferences = notif_prefs;

            let cred_account = account_disc_for_policy.as_deref().unwrap_or("default");

            if is_usage_refresh_success {
                let _ = policy.credential_tracker.handle_outcome(
                    id,
                    cred_account,
                    &CredentialFetchOutcome::fresh_success(source_label),
                    cred_enabled,
                );
                if let Some(snap) = &snapshot_opt {
                    let codex_guard = (id.as_str() == "codex").then(|| {
                        let identity_snap = snap.identity_for(id).or(snap.identity.as_ref());
                        let codex_identity = if let Some(acct_id) =
                            identity_snap.and_then(|i| i.account_id.as_deref())
                        {
                            CodexIdentity::ProviderAccount {
                                id: acct_id.to_string(),
                            }
                        } else if let Some(email) =
                            identity_snap.and_then(|i| i.account_email.as_deref())
                        {
                            CodexIdentity::EmailOnly {
                                normalized_email: email.to_string(),
                            }
                        } else {
                            CodexIdentity::ProviderAccount {
                                id: "default".to_string(),
                            }
                        };
                        let email_or_key = identity_snap
                            .and_then(|i| i.account_email.clone())
                            .or_else(|| account_disc_for_policy.clone())
                            .or_else(|| Some("default@codex.local".to_string()));
                        CodexAccountScopedRefreshGuard::new(
                            CodexActiveSource::LiveSystem,
                            codex_identity,
                            email_or_key,
                            Some("live".to_string()),
                        )
                    });
                    let codex_session_owner = codex_guard
                        .as_ref()
                        .and_then(CodexSessionQuotaOwnerKey::from_refresh_guard);
                    let codex_reset_owner = codex_guard
                        .as_ref()
                        .and_then(CodexLimitResetOwnerKey::from_refresh_guard);

                    let (session_post, session_restored_pending, _) = policy
                        .handle_session_quota_transition(
                            id,
                            snap,
                            account_disc_for_policy.as_deref(),
                            codex_session_owner.as_ref(),
                            hooks_cfg.enabled,
                            now,
                        );
                    if let Some(post) = session_post {
                        emitted_posts.push(post);
                    }
                    let (_, warn_posts) = policy.handle_quota_warning_transitions(
                        id,
                        snap,
                        account_disc_for_policy.as_deref(),
                        false,
                    );
                    emitted_posts.extend(warn_posts);
                    let reset_posts = policy.handle_limit_reset_transitions(
                        id,
                        snap,
                        account_disc_for_policy.as_deref(),
                        codex_reset_owner.as_ref(),
                        session_restored_pending,
                    );
                    emitted_posts.extend(reset_posts);
                }
            } else if let Some(FetchError::Classified(classified)) = fetch_err {
                let cred_err = match classified.kind {
                    ProviderErrorKind::AuthenticationExpired => {
                        Some(ProviderCredentialError::Classified {
                            kind: ProviderFetchErrorKind::AuthenticationExpired,
                            message: classified.message.clone(),
                        })
                    }
                    ProviderErrorKind::MissingCredential => {
                        Some(ProviderCredentialError::Classified {
                            kind: ProviderFetchErrorKind::MissingCredential,
                            message: classified.message.clone(),
                        })
                    }
                    _ => None,
                };
                if let Some(err) = cred_err
                    && let CredentialNotificationAction::Post(post) =
                        policy.credential_tracker.handle_outcome(
                            id,
                            cred_account,
                            &CredentialFetchOutcome::Failure(err),
                            cred_enabled,
                        )
                {
                    emitted_posts.push(post);
                }
            }
            if let Some(path) = self
                .notification_state_path
                .read()
                .expect("notification_state_path lock")
                .clone()
                && let Ok(bytes) = serde_json::to_vec_pretty(&policy.export_persisted_state())
            {
                let _ = crate::private_file::write_private(
                    &path,
                    &bytes,
                    &crate::private_file::WriteHooks::default(),
                );
            }
        }

        for post in emitted_posts {
            self.transition_sink.on_notification(&post);
            let _ = self.notifier.notify_post(&post);
        }

        let dispatches = self
            .hook_detector
            .lock()
            .expect("hook_detector lock")
            .evaluate(&observation, &hooks_cfg, now);
        for dispatch in &dispatches {
            self.transition_sink.on_hook_dispatch(dispatch);
        }
        if let Some(runner) = &self.hook_runner
            && !dispatches.is_empty()
        {
            let forwarded_env: BTreeMap<String, String> = {
                let base = self.base_environment.read().expect("base_env lock");
                base.iter()
                    .filter(|(k, _)| FORWARDED_ENVIRONMENT_KEYS.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.expose_secret().to_string()))
                    .collect()
            };
            for dispatch in &dispatches {
                let _ = runner
                    .dispatch(
                        &dispatch.event,
                        &hooks_cfg,
                        dispatch.account_discriminator.as_deref(),
                        &forwarded_env,
                        now,
                    )
                    .await;
            }
        }
    }

    // -------------------------------------------------------------------------
    // Adaptive & fixed refresh timer scheduler (`UsageStore+AdaptiveRefresh.swift`)
    // -------------------------------------------------------------------------

    /// Returns the number of completed batch refreshes since runtime creation.
    pub fn completed_refresh_count(&self) -> u64 {
        self.completed_refresh_count.load(Ordering::SeqCst)
    }

    /// Returns whether a batch refresh is currently in flight.
    pub fn is_refreshing(&self) -> bool {
        self.batch_state
            .lock()
            .expect("batch_state lock")
            .is_refreshing
    }

    /// Overrides the in-flight batch flag for testing opportunistic timer coalescing.
    pub fn set_refreshing_for_testing(&self, refreshing: bool) {
        let mut batch = self.batch_state.lock().expect("batch_state lock");
        batch.is_refreshing = refreshing;
        let _ = self.batch_idle_tx.send(!refreshing);
    }

    /// Returns the last recorded menu-open timestamp, if any.
    pub fn last_menu_opened_at(&self) -> Option<Timestamp> {
        self.adaptive_state
            .lock()
            .expect("adaptive_state lock")
            .last_menu_opened_at
    }

    /// Returns the last recorded coding-activity timestamp, if any.
    pub fn last_coding_activity_at(&self) -> Option<Timestamp> {
        self.adaptive_state
            .lock()
            .expect("adaptive_state lock")
            .last_coding_activity_at
    }

    /// Returns the currently scheduled background refresh fire timestamp, if any.
    pub fn scheduled_fire_at(&self) -> Option<Timestamp> {
        self.adaptive_state
            .lock()
            .expect("adaptive_state lock")
            .scheduled_fire_at
    }

    /// Overrides the scheduled fire timestamp for deterministic adaptive-timer tests.
    pub fn set_scheduled_fire_at_for_testing(&self, fire_at: Option<Timestamp>) {
        self.adaptive_state
            .lock()
            .expect("adaptive_state lock")
            .scheduled_fire_at = fire_at;
    }

    /// Returns the effective fixed refresh interval computed when starting a fixed timer.
    pub fn fixed_refresh_interval(&self) -> Option<Duration> {
        self.adaptive_state
            .lock()
            .expect("adaptive_state lock")
            .fixed_refresh_interval
    }

    /// Returns the effective adaptive refresh interval computed on the latest adaptive tick.
    pub fn adaptive_computed_interval(&self) -> Option<Duration> {
        self.adaptive_state
            .lock()
            .expect("adaptive_state lock")
            .adaptive_computed_interval
    }

    /// Returns the active timer sleep override duration, if configured.
    pub fn timer_sleep_override(&self) -> Option<Duration> {
        self.adaptive_state
            .lock()
            .expect("adaptive_state lock")
            .sleep_override
    }

    /// Sets the timer sleep override duration for deterministic timer tests.
    pub fn set_timer_sleep_override(&self, sleep_override: Option<Duration>) {
        self.adaptive_state
            .lock()
            .expect("adaptive_state lock")
            .sleep_override = sleep_override;
    }

    /// Evaluates the adaptive refresh decision at `now` if the current cadence is adaptive,
    /// returning `None` when the configured cadence is manual or a fixed interval.
    pub fn evaluate_adaptive_cadence(&self, now: Timestamp) -> Option<AdaptiveRefreshDecision> {
        let config = self.load_config();
        let prefs = AppPreferences::from_config(&config);
        if !prefs.refresh_frequency.uses_adaptive_policy() {
            return None;
        }
        let adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
        let coding_activity = if prefs.refresh_frequency.uses_coding_activity() {
            adaptive.last_coding_activity_at
        } else {
            None
        };
        let input = AdaptiveRefreshInput::new(
            now,
            adaptive.last_menu_opened_at,
            coding_activity,
            adaptive.is_low_power_mode,
            adaptive.thermal_pressure,
        );
        Some(AdaptiveRefreshPolicyCore.next_delay(&input))
    }

    /// Computes the next background timer delay from `now` for both fixed and adaptive cadences
    /// (clamped by [`BackgroundWorkPowerPolicy`] when low-power mode is active), returning `None`
    /// when cadence is `Manual`.
    pub fn next_refresh_delay(&self, now: Timestamp) -> Option<Duration> {
        let config = self.load_config();
        let prefs = AppPreferences::from_config(&config);
        if let Some(fixed) = prefs.refresh_frequency.duration() {
            let mut adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
            let effective = BackgroundWorkPowerPolicy::automatic_interval(
                Some(fixed),
                adaptive.is_low_power_mode,
            )
            .unwrap_or(fixed);
            adaptive.fixed_refresh_interval = Some(effective);
            adaptive.scheduled_fire_at = Some(add_duration(now, effective));
            return Some(effective);
        }
        if !prefs.refresh_frequency.uses_adaptive_policy() {
            return None;
        }
        let mut adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
        let coding_activity = if prefs.refresh_frequency.uses_coding_activity() {
            adaptive.last_coding_activity_at
        } else {
            None
        };
        let input = AdaptiveRefreshInput::new(
            now,
            adaptive.last_menu_opened_at,
            coding_activity,
            adaptive.is_low_power_mode,
            adaptive.thermal_pressure,
        );
        let decision = AdaptiveRefreshPolicyCore.next_delay(&input);
        let effective = BackgroundWorkPowerPolicy::automatic_interval(
            Some(decision.delay),
            adaptive.is_low_power_mode,
        )
        .unwrap_or(decision.delay);
        adaptive.adaptive_computed_interval = Some(effective);
        adaptive.scheduled_fire_at = Some(add_duration(now, effective));
        Some(effective)
    }

    /// Computes this tick's adaptive sleep duration, updating `adaptive_computed_interval` and
    /// `scheduled_fire_at` while honoring any configured test sleep override (`UsageStore.nextAdaptiveTimerSleepDuration`).
    pub fn next_adaptive_timer_sleep_duration(&self) -> Option<Duration> {
        let config = self.load_config();
        let prefs = AppPreferences::from_config(&config);
        if !prefs.refresh_frequency.uses_adaptive_policy() {
            return None;
        }
        let now = self.now();
        let mut adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
        let coding_activity = if prefs.refresh_frequency.uses_coding_activity() {
            adaptive.last_coding_activity_at
        } else {
            None
        };
        let input = AdaptiveRefreshInput::new(
            now,
            adaptive.last_menu_opened_at,
            coding_activity,
            adaptive.is_low_power_mode,
            adaptive.thermal_pressure,
        );
        let decision = AdaptiveRefreshPolicyCore.next_delay(&input);
        let effective = BackgroundWorkPowerPolicy::automatic_interval(
            Some(decision.delay),
            adaptive.is_low_power_mode,
        )
        .unwrap_or(decision.delay);
        adaptive.adaptive_computed_interval = Some(effective);
        adaptive.scheduled_fire_at = Some(add_duration(now, effective));
        Some(adaptive.sleep_override.unwrap_or(effective))
    }

    /// Records that the user opened the menu or panel at `now`. Returns `true` if an adaptive
    /// timer was advanced to fire sooner.
    pub fn note_menu_opened(&self, now: Timestamp) -> bool {
        let config = self.load_config();
        let prefs = AppPreferences::from_config(&config);
        let (advanced, restart) = {
            let mut adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
            adaptive.last_menu_opened_at = Some(now);
            let adv =
                Self::recompute_adaptive_schedule(&mut adaptive, prefs.refresh_frequency, now);
            (adv, adv && adaptive.timer_active)
        };
        if restart {
            self.restart_timer_internal();
        }
        advanced
    }

    /// Records observed local coding activity at `observed_at` (using `observed_at` as `now`)
    /// for `AdaptiveAgentAware` mode. Returns `true` if the adaptive timer was advanced to fire sooner.
    pub fn note_coding_activity_observed(&self, observed_at: Timestamp) -> bool {
        self.note_coding_activity_observed_at(observed_at, observed_at)
    }

    /// Records observed local coding activity at `observed_at` evaluated against `now`
    /// (`UsageStore.noteCodingActivityObserved(at:now:)`).
    ///
    /// Ignored unless the configured cadence uses coding activity (`AdaptiveAgentAware`), and
    /// retains `observed_at` only if it is newer than the previously recorded observation.
    pub fn note_coding_activity_observed_at(&self, observed_at: Timestamp, now: Timestamp) -> bool {
        let config = self.load_config();
        let prefs = AppPreferences::from_config(&config);
        if !prefs.refresh_frequency.uses_coding_activity() {
            return false;
        }
        let (advanced, restart) = {
            let mut adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
            if adaptive
                .last_coding_activity_at
                .is_none_or(|prev| observed_at > prev)
            {
                adaptive.last_coding_activity_at = Some(observed_at);
            }
            let adv =
                Self::recompute_adaptive_schedule(&mut adaptive, prefs.refresh_frequency, now);
            (adv, adv && adaptive.timer_active)
        };
        if restart {
            self.restart_timer_internal();
        }
        advanced
    }

    /// Clears the coding activity observation timestamp.
    pub fn clear_coding_activity_observation(&self) -> bool {
        let mut adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
        adaptive.last_coding_activity_at.take().is_some()
    }

    /// Updates low-power mode state at `now`.
    pub fn set_low_power_mode(&self, enabled: bool, now: Timestamp) -> bool {
        let config = self.load_config();
        let prefs = AppPreferences::from_config(&config);
        let (advanced, restart) = {
            let mut adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
            adaptive.is_low_power_mode = enabled;
            let adv =
                Self::recompute_adaptive_schedule(&mut adaptive, prefs.refresh_frequency, now);
            let needs_fixed_restart =
                adaptive.timer_active && prefs.refresh_frequency.duration().is_some();
            (adv, (adv && adaptive.timer_active) || needs_fixed_restart)
        };
        if restart {
            self.restart_timer_internal();
        }
        advanced
    }

    /// Updates system thermal pressure at `now`.
    pub fn set_thermal_pressure(&self, pressure: ThermalPressure, now: Timestamp) -> bool {
        let config = self.load_config();
        let prefs = AppPreferences::from_config(&config);
        let (advanced, restart) = {
            let mut adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
            adaptive.thermal_pressure = pressure;
            let adv =
                Self::recompute_adaptive_schedule(&mut adaptive, prefs.refresh_frequency, now);
            (adv, adv && adaptive.timer_active)
        };
        if restart {
            self.restart_timer_internal();
        }
        advanced
    }

    fn recompute_adaptive_schedule(
        adaptive: &mut AdaptiveRuntimeState,
        frequency: RefreshFrequency,
        now: Timestamp,
    ) -> bool {
        if !frequency.uses_adaptive_policy() {
            return false;
        }
        let coding_activity = if frequency.uses_coding_activity() {
            adaptive.last_coding_activity_at
        } else {
            None
        };
        let input = AdaptiveRefreshInput::new(
            now,
            adaptive.last_menu_opened_at,
            coding_activity,
            adaptive.is_low_power_mode,
            adaptive.thermal_pressure,
        );
        let decision = AdaptiveRefreshPolicyCore.next_delay(&input);
        let effective = BackgroundWorkPowerPolicy::automatic_interval(
            Some(decision.delay),
            adaptive.is_low_power_mode,
        )
        .unwrap_or(decision.delay);
        let candidate = add_duration(now, effective);
        if should_advance_adaptive_timer(adaptive.scheduled_fire_at, candidate) {
            adaptive.adaptive_computed_interval = Some(effective);
            adaptive.scheduled_fire_at = Some(candidate);
            true
        } else {
            false
        }
    }

    /// Starts (or restarts) the background refresh timer according to the current `RefreshFrequency`
    /// (`UsageStore.startTimer()`).
    ///
    /// - `Manual`: stops any running timer and does not schedule recurring ticks.
    /// - Fixed intervals (`1m`, `2m`, `5m`, `15m`, `30m`): clamps the interval via
    ///   [`BackgroundWorkPowerPolicy::automatic_interval`] when low-power mode is active and runs
    ///   an interval-aligned loop (`run_fixed_refresh_timer`).
    /// - Adaptive modes (`Adaptive`, `AdaptiveAgentAware`): recomputes the delay before each tick
    ///   from live menu-open, coding-activity, low-power, and thermal signals.
    pub fn start_timer(self: &Arc<Self>) {
        *self.weak_self.lock().expect("weak_self lock") = Some(Arc::downgrade(self));
        self.restart_timer_internal();
    }

    /// Sets the instance-scoped timer sleep override and restarts the background refresh timer
    /// (`UsageStore.restartTimerWithSleepOverrideForTesting`).
    pub fn restart_timer_with_sleep_override(self: &Arc<Self>, sleep_override: Option<Duration>) {
        *self.weak_self.lock().expect("weak_self lock") = Some(Arc::downgrade(self));
        self.adaptive_state
            .lock()
            .expect("adaptive_state lock")
            .sleep_override = sleep_override;
        self.restart_timer_internal();
    }

    /// Stops any active background refresh timer without aborting an already-started batch refresh.
    pub fn stop_timer(&self) {
        if let Some(cancel) = self.timer_cancel.lock().expect("timer_cancel lock").take() {
            cancel.cancel();
        }
        let _ = self.timer_task.lock().expect("timer_task lock").take();
        let mut adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
        adaptive.timer_active = false;
        adaptive.scheduled_fire_at = None;
        adaptive.fixed_refresh_interval = None;
        adaptive.adaptive_computed_interval = None;
    }

    fn restart_timer_internal(&self) {
        if let Some(cancel) = self.timer_cancel.lock().expect("timer_cancel lock").take() {
            cancel.cancel();
        }
        let _ = self.timer_task.lock().expect("timer_task lock").take();

        let config = self.load_config();
        let prefs = AppPreferences::from_config(&config);
        {
            let mut adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
            adaptive.scheduled_fire_at = None;
            adaptive.fixed_refresh_interval = None;
            adaptive.adaptive_computed_interval = None;
            if prefs.refresh_frequency == RefreshFrequency::Manual {
                adaptive.timer_active = false;
                return;
            }
            adaptive.timer_active = true;
        }

        let Some(weak) = self.weak_self.lock().expect("weak_self lock").clone() else {
            return;
        };

        let cancel = CancellationToken::new();
        if prefs.refresh_frequency.uses_adaptive_policy() {
            let Some(first_sleep) = self.next_adaptive_timer_sleep_duration() else {
                return;
            };
            let task_cancel = cancel.clone();
            let handle = tokio::spawn(async move {
                let mut next_sleep = first_sleep;
                loop {
                    tokio::select! {
                        biased;
                        () = task_cancel.cancelled() => return,
                        () = tokio::time::sleep(next_sleep) => {}
                    }
                    if task_cancel.is_cancelled() {
                        return;
                    }
                    let Some(runtime) = weak.upgrade() else {
                        return;
                    };
                    let batch_runtime = Arc::clone(&runtime);
                    let _ = tokio::spawn(async move {
                        let _ = batch_runtime
                            .refresh_batch(RefreshOptions::background())
                            .await;
                    })
                    .await;
                    if task_cancel.is_cancelled() {
                        return;
                    }
                    let Some(sleep_dur) = runtime.next_adaptive_timer_sleep_duration() else {
                        return;
                    };
                    next_sleep = sleep_dur;
                    drop(runtime);
                }
            });
            *self.timer_cancel.lock().expect("timer_cancel lock") = Some(cancel);
            *self.timer_task.lock().expect("timer_task lock") = Some(handle);
            return;
        }

        let Some(raw_interval) = prefs.refresh_frequency.duration() else {
            return;
        };
        let now = self.now();
        let (interval, sleep_override) = {
            let mut adaptive = self.adaptive_state.lock().expect("adaptive_state lock");
            let effective = BackgroundWorkPowerPolicy::automatic_interval(
                Some(raw_interval),
                adaptive.is_low_power_mode,
            )
            .unwrap_or(raw_interval);
            adaptive.fixed_refresh_interval = Some(effective);
            adaptive.scheduled_fire_at = Some(add_duration(now, effective));
            (effective, adaptive.sleep_override)
        };

        let task_cancel = cancel.clone();
        let start_instant = std::time::Instant::now();
        let handle = tokio::spawn(async move {
            Self::run_fixed_refresh_timer(
                interval,
                sleep_override,
                task_cancel,
                move || start_instant.elapsed(),
                |dur| async move {
                    tokio::time::sleep(dur).await;
                    true
                },
                move || {
                    let weak_inner = weak.clone();
                    async move {
                        if let Some(runtime) = weak_inner.upgrade() {
                            let _ = tokio::spawn(async move {
                                let _ = runtime.refresh_batch(RefreshOptions::background()).await;
                            })
                            .await;
                        }
                    }
                },
            )
            .await;
        });
        *self.timer_cancel.lock().expect("timer_cancel lock") = Some(cancel);
        *self.timer_task.lock().expect("timer_task lock") = Some(handle);
    }

    /// Runs an interval-aligned fixed refresh timer loop (`UsageStore.runFixedRefreshTimer`),
    /// advancing each tick from the previous scheduled tick time via [`next_fixed_timer_scheduled_at`]
    /// so slow refreshes skip missed ticks rather than drifting or overlapping.
    pub async fn run_fixed_refresh_timer<NowFn, SleepFn, SleepFut, RefreshFn, RefreshFut>(
        interval: Duration,
        sleep_override: Option<Duration>,
        cancel: CancellationToken,
        mut now_fn: NowFn,
        mut sleep_fn: SleepFn,
        mut refresh_fn: RefreshFn,
    ) where
        NowFn: FnMut() -> Duration,
        SleepFn: FnMut(Duration) -> SleepFut,
        SleepFut: Future<Output = bool>,
        RefreshFn: FnMut() -> RefreshFut,
        RefreshFut: Future<Output = ()>,
    {
        if interval.is_zero() {
            return;
        }
        let mut scheduled_at = now_fn().saturating_add(interval);
        while !cancel.is_cancelled() {
            let current = now_fn();
            let computed_sleep = scheduled_at.checked_sub(current).unwrap_or(Duration::ZERO);
            let sleep_dur = sleep_override.unwrap_or(computed_sleep);
            let completed = tokio::select! {
                biased;
                () = cancel.cancelled() => false,
                ok = sleep_fn(sleep_dur) => ok,
            };
            if !completed || cancel.is_cancelled() {
                return;
            }
            refresh_fn().await;
            if cancel.is_cancelled() {
                return;
            }
            scheduled_at = next_fixed_timer_scheduled_at(scheduled_at, now_fn(), interval);
        }
    }

    // -------------------------------------------------------------------------
    // Documented IPC v1 Intent Entry Points (`w-mac-3`)
    // -------------------------------------------------------------------------

    /// Returns the current [`EngineSnapshot`] for [`ClientMessage::GetSnapshot`].
    pub fn get_snapshot(&self) -> EngineSnapshot {
        self.current_snapshot()
    }

    /// Builds a [`SettingsPage`] for `scope` (`ClientMessage::GetSettings`), querying
    /// [`SecretStore::presence`] in background mode so it never triggers an interactive unlock.
    pub async fn get_settings(&self, scope: &SettingsScope) -> Result<SettingsPage, IpcError> {
        let config = self.load_config();
        let call = BrokerCall {
            interaction: FetchInteraction::Background,
            cancel: CancellationToken::new(),
            request_id: "settings-presence".to_string(),
        };
        let presence: SecretPresence = self.secret_store.presence(&config, call).await;
        let registry = self.registry.read().expect("registry lock").clone();
        build_settings_page(&registry, &config, &presence, scope).map_err(map_settings_error)
    }

    /// Applies a [`ClientMessage::SetSetting`] mutation atomically through [`ConfigStore`],
    /// emits [`SettingsScope`] on the settings-changed channel, and refreshes affected providers.
    pub async fn set_setting(
        &self,
        id: &SettingId,
        value: &serde_json::Value,
    ) -> Result<SettingsScope, IpcError> {
        let registry = self.registry.read().expect("registry lock").clone();
        let id_clone = id.clone();
        let value_clone = value.clone();
        let mutation_outcome = Mutex::new(None);

        self.config_store
            .update(|cfg| {
                let mut draft = cfg.clone();
                let res = apply_setting_mutation(&registry, &mut draft, &id_clone, &value_clone);
                if res.is_ok() {
                    *cfg = draft;
                }
                *mutation_outcome.lock().expect("mutation_outcome lock") = Some(res);
            })
            .map_err(|_| IpcError::new(ErrorCode::InternalError))?;

        let (scope, affected_provider) = mutation_outcome
            .into_inner()
            .expect("mutation_outcome lock")
            .expect("update closure executed")
            .map_err(map_settings_error)?;

        let _ = self.settings_changed_tx.send(scope.clone());
        let _ = self.pushes_tx.send(ServerMessage::SettingsChanged {
            scope: scope.clone(),
        });

        if matches!(
            id.as_str(),
            "app.refresh.cadence" | "refreshCadence" | "refreshInterval" | "refreshIntervalSeconds"
        ) && self.weak_self.lock().expect("weak_self lock").is_some()
        {
            self.restart_timer_internal();
        }

        if let Some(pid) = affected_provider {
            let config = self.load_config();
            if !registry.is_provider_enabled(&config, &pid) {
                self.clear_disabled_provider(&pid);
                self.publish_current_snapshot();
            } else {
                self.refresh_provider(pid, RefreshOptions::settings_change())
                    .await;
            }
        } else {
            self.publish_current_snapshot();
        }

        Ok(scope)
    }

    /// Executes a [`ClientMessage::RunAction`] intent (`refreshAll`, `refreshProvider.<id>`,
    /// `menuOpened`, `resetUsage`, `clearCookieCache`, `clearCookieCache.<id>`, `clearCostCache`,
    /// `diagnose`, `diagnose.<id>`).
    pub async fn run_action(&self, id: &ActionId, confirm: Option<bool>) -> Result<(), IpcError> {
        self.run_action_with_payload(id, confirm).await.map(|_| ())
    }

    /// Executes a [`ClientMessage::RunAction`] intent and returns an optional JSON payload for
    /// [`ServerMessage::Ok`].
    pub async fn run_action_with_payload(
        &self,
        id: &ActionId,
        _confirm: Option<bool>,
    ) -> Result<Option<serde_json::Value>, IpcError> {
        match id.as_str() {
            "refreshAll" => {
                self.refresh_required(RefreshOptions::user_initiated())
                    .await;
                Ok(None)
            }
            "menuOpened" | "noteMenuOpened" => {
                self.note_menu_opened(self.now());
                Ok(None)
            }
            "refreshProvider" => Err(IpcError::with_static_message(
                ErrorCode::InvalidRequest,
                "refreshProvider requires a target provider id",
            )),
            "resetUsage" => {
                {
                    let mut states = self.providers.lock().expect("providers lock");
                    for state in states.values_mut() {
                        state.snapshot = None;
                        state.last_known_reset_snapshot = None;
                        state.last_error = None;
                        state.failure_gate.reset();
                        state.refresh_state = RefreshState::Fresh;
                        state.kept_snapshot = false;
                    }
                }
                self.publish_current_snapshot();
                Ok(None)
            }
            "clearCookieCache" => {
                let summary = self.cookie_cache.clear_all_detailed();
                Ok(Some(serde_json::json!({
                    "cleared": summary.cleared_count,
                    "failed": summary.failed_count,
                })))
            }
            "clearCostCache" => {
                let (cleared, error) = self.clear_cost_cache();
                Ok(Some(serde_json::json!({
                    "cleared": cleared,
                    "error": error,
                })))
            }
            "diagnose" => {
                let batch = self.diagnose_all_providers(true).await;
                let payload = serde_json::to_value(batch)
                    .map_err(|_| IpcError::new(ErrorCode::InternalError))?;
                Ok(Some(payload))
            }
            other => {
                if let Some(pid_str) = other.strip_prefix("clearCookieCache.") {
                    let pid =
                        ProviderId::new(pid_str).map_err(|_| IpcError::new(ErrorCode::NotFound))?;
                    if self
                        .registry
                        .read()
                        .expect("registry lock")
                        .descriptor(&pid)
                        .is_none()
                    {
                        return Err(IpcError::new(ErrorCode::NotFound));
                    }
                    let summary = self.cookie_cache.clear_all_scopes_detailed(&pid);
                    return Ok(Some(serde_json::json!({
                        "cleared": summary.cleared_count,
                        "failed": summary.failed_count,
                    })));
                }
                if let Some(pid_str) = other.strip_prefix("diagnose.") {
                    let pid =
                        ProviderId::new(pid_str).map_err(|_| IpcError::new(ErrorCode::NotFound))?;
                    let export = self
                        .diagnose_provider(&pid, true)
                        .await
                        .ok_or_else(|| IpcError::new(ErrorCode::NotFound))?;
                    let payload = serde_json::to_value(export)
                        .map_err(|_| IpcError::new(ErrorCode::InternalError))?;
                    return Ok(Some(payload));
                }
                let target = other.strip_prefix("refreshProvider.").or_else(|| {
                    other
                        .strip_prefix("providers.")
                        .and_then(|rest| rest.strip_suffix(".refresh"))
                });
                if let Some(pid_str) = target {
                    let pid =
                        ProviderId::new(pid_str).map_err(|_| IpcError::new(ErrorCode::NotFound))?;
                    if self
                        .registry
                        .read()
                        .expect("registry lock")
                        .descriptor(&pid)
                        .is_none()
                    {
                        return Err(IpcError::new(ErrorCode::NotFound));
                    }
                    self.refresh_provider(pid, RefreshOptions::user_initiated())
                        .await;
                    Ok(None)
                } else {
                    Err(IpcError::new(ErrorCode::NotFound))
                }
            }
        }
    }

    /// Clears the persisted cost usage cache directory (`<cache_root>/cost-usage`) without
    /// touching sibling directories such as `codex-sqlite-copies` (`CLICacheCommand.swift:55-68`).
    pub fn clear_cost_cache(&self) -> (usize, Option<String>) {
        let root = self
            .cost_cache_root
            .read()
            .expect("cost_cache_root lock")
            .clone()
            .unwrap_or_else(crate::cost::store::default_cache_root);
        if root.as_os_str().is_empty() {
            return (0, None);
        }
        let cache_dir = root.join("cost-usage");
        if !cache_dir.exists() {
            return (0, None);
        }
        match std::fs::remove_dir_all(&cache_dir) {
            Ok(()) => (1, None),
            Err(err) => (0, Some(err.to_string())),
        }
    }

    /// Executes a live fetch outcome for `id` without mutating the daemon's cached snapshot state,
    /// used by `ariadusage diagnose`, `ariadusage cookie refresh`, and standalone CLI `usage`.
    pub async fn fetch_provider_for_cli(
        &self,
        id: &ProviderId,
        runtime_kind: FetchRuntime,
        interaction: FetchInteraction,
    ) -> Option<(
        ResolvedProviderSpec,
        ProviderPlanInputs,
        ariadusage_core::pipeline::FetchOutcome,
    )> {
        let config = self.load_config();
        let prefs = AppPreferences::from_config(&config);
        let request_state = self.coordinator.begin_replacing_request(id.clone()).state;
        let inputs = self
            .resolve_plan_inputs(id, &config, &prefs, interaction, &request_state)
            .await;
        let spec = self
            .registry
            .read()
            .expect("registry lock")
            .resolve_spec(id, &config, &inputs)?;
        let req_seq = self.next_request_seq.fetch_add(1, Ordering::Relaxed);
        let cx = FetchContext {
            provider: id.clone(),
            runtime: runtime_kind,
            interaction,
            phase: FetchPhase::Regular,
            request_id: format!("cli-{}-{req_seq}", id.as_str()),
            source_mode: spec.source_mode,
            env: spec.environment.clone(),
            include_credits: prefs.show_optional_credits_and_extra_usage,
            selected_token_account: spec.selected_account.as_ref().map(|a| a.id.clone()),
            cancel: CancellationToken::new(),
        };
        let outcome = fetch_outcome(id, spec.plan.as_ref(), &cx, self.sleeper.as_ref()).await;
        self.coordinator.complete(&request_state, id, false);
        Some((spec, inputs, outcome))
    }

    /// Exports a [`ariadusage_core::pipeline::DiagnosticExport`] for a single provider.
    ///
    /// Emails, tokens, cookies, and account identifiers are always redacted in CLI/IPC exports.
    pub async fn diagnose_provider(
        &self,
        id: &ProviderId,
        _redact: bool,
    ) -> Option<ariadusage_core::pipeline::DiagnosticExport> {
        use ariadusage_core::pipeline::{
            DiagnosticAuthSummary, DiagnosticExportBuilder, DiagnosticExportBuilderInput,
        };
        let (spec, inputs, outcome) = self
            .fetch_provider_for_cli(id, FetchRuntime::Cli, FetchInteraction::UserInitiated)
            .await?;

        let mut modes = Vec::new();
        if spec.selected_account.is_some() {
            modes.push("tokenAccount".to_string());
        }
        if inputs.api_key.is_some() {
            modes.push("api".to_string());
        }
        if id.as_str() == "claude" {
            let has_admin_env = spec
                .environment
                .get("ANTHROPIC_ADMIN_API_KEY")
                .or_else(|| spec.environment.get("ANTHROPIC_ADMIN_KEY"))
                .is_some_and(|v| !v.expose_secret().trim().is_empty());
            if has_admin_env && !modes.contains(&"api".to_string()) {
                modes.push("api".to_string());
            }
        }
        if inputs.manual_cookie_header.is_some() && !modes.contains(&"web".to_string()) {
            modes.push("web".to_string());
        }

        let auth = DiagnosticAuthSummary::new(!modes.is_empty(), modes);

        Some(DiagnosticExportBuilder::build(
            DiagnosticExportBuilderInput {
                provider: id,
                descriptor: spec.descriptor,
                outcome: &outcome,
                source_mode: spec.source_mode,
                api_region: None,
                auth,
                app_version: Some(self.engine_version.clone()),
                timestamp: Some(self.now()),
            },
        ))
    }

    /// Exports a [`ariadusage_core::pipeline::DiagnosticBatchExport`] for all first-party providers.
    pub async fn diagnose_all_providers(
        &self,
        redact: bool,
    ) -> ariadusage_core::pipeline::DiagnosticBatchExport {
        let descriptors: Vec<&'static ProviderDescriptor> = self
            .registry
            .read()
            .expect("registry lock")
            .descriptors()
            .iter()
            .collect();
        let mut exports = Vec::with_capacity(descriptors.len());
        for desc in descriptors {
            if let Some(export) = self.diagnose_provider(&desc.id, redact).await {
                exports.push(export);
            }
        }
        ariadusage_core::pipeline::DiagnosticBatchExport::new(self.now(), exports)
    }

    /// Dispatches a single [`ClientMessage`] intent and returns the resulting [`ServerMessage`]
    /// responses to send back to the client.
    pub async fn handle_client_intent(&self, message: ClientMessage) -> Vec<ServerMessage> {
        match message {
            ClientMessage::Hello { protocols, id, .. } => {
                if let Some(proto) = negotiate_protocol(&protocols) {
                    vec![ServerMessage::Welcome {
                        protocol: proto.to_string(),
                        engine_version: self.engine_version.clone(),
                        capabilities: vec![
                            "snapshot".to_string(),
                            "settings".to_string(),
                            "notices".to_string(),
                        ],
                        id,
                    }]
                } else {
                    vec![ServerMessage::error(
                        id,
                        IpcError::with_static_message(
                            ErrorCode::UnsupportedMessage,
                            "no compatible protocol version",
                        ),
                    )]
                }
            }
            ClientMessage::Subscribe { id, .. } => vec![
                ServerMessage::ok(id),
                ServerMessage::Snapshot {
                    snapshot: Box::new(self.get_snapshot()),
                },
            ],
            ClientMessage::GetSnapshot { id: _ } => vec![ServerMessage::Snapshot {
                snapshot: Box::new(self.get_snapshot()),
            }],
            ClientMessage::Refresh { provider, id } => {
                if let Some(pid) = provider {
                    if self
                        .registry
                        .read()
                        .expect("registry lock")
                        .descriptor(&pid)
                        .is_none()
                    {
                        return vec![ServerMessage::error(id, IpcError::new(ErrorCode::NotFound))];
                    }
                    self.refresh_provider(pid, RefreshOptions::user_initiated())
                        .await;
                } else {
                    self.refresh_required(RefreshOptions::user_initiated())
                        .await;
                }
                vec![
                    ServerMessage::ok(id),
                    ServerMessage::Snapshot {
                        snapshot: Box::new(self.get_snapshot()),
                    },
                ]
            }
            ClientMessage::GetSettings { scope, id } => match self.get_settings(&scope).await {
                Ok(page) => match serde_json::to_value(page) {
                    Ok(payload) => vec![ServerMessage::ok_with_payload(id, payload)],
                    Err(_) => vec![ServerMessage::error(
                        id,
                        IpcError::new(ErrorCode::InternalError),
                    )],
                },
                Err(err) => vec![ServerMessage::error(id, err)],
            },
            ClientMessage::SetSetting { id, value } => match self.set_setting(&id, &value).await {
                Ok(_) => vec![ServerMessage::ok(None)],
                Err(err) => vec![ServerMessage::error(None, err)],
            },
            ClientMessage::RunAction { id, confirm } => {
                match self.run_action_with_payload(&id, confirm).await {
                    Ok(Some(payload)) => vec![ServerMessage::ok_with_payload(None, payload)],
                    Ok(None) => vec![ServerMessage::ok(None)],
                    Err(err) => vec![ServerMessage::error(None, err)],
                }
            }
            ClientMessage::SetSecret { .. } => vec![ServerMessage::error(
                None,
                IpcError::with_static_message(
                    ErrorCode::UnsupportedMessage,
                    "secrets must be set via ariadusage secret set",
                ),
            )],
        }
    }
}

impl IpcBackend for EngineRuntime {
    fn subscribe_pushes(&self) -> broadcast::Receiver<ServerMessage> {
        self.pushes_tx.subscribe()
    }

    fn current_snapshot(&self) -> Pin<Box<dyn Future<Output = EngineSnapshot> + Send + '_>> {
        Box::pin(async move { self.get_snapshot() })
    }

    fn refresh(
        &self,
        provider: Option<ProviderId>,
    ) -> Pin<Box<dyn Future<Output = Result<EngineSnapshot, IpcError>> + Send + '_>> {
        Box::pin(async move {
            if let Some(pid) = provider {
                if self
                    .registry
                    .read()
                    .expect("registry lock")
                    .descriptor(&pid)
                    .is_none()
                {
                    return Err(IpcError::new(ErrorCode::NotFound));
                }
                self.refresh_provider(pid, RefreshOptions::user_initiated())
                    .await;
            } else {
                self.refresh_required(RefreshOptions::user_initiated())
                    .await;
            }
            Ok(self.get_snapshot())
        })
    }

    fn get_settings(
        &self,
        scope: SettingsScope,
    ) -> Pin<Box<dyn Future<Output = Result<SettingsPage, IpcError>> + Send + '_>> {
        Box::pin(async move { self.get_settings(&scope).await })
    }

    fn set_setting(
        &self,
        id: SettingId,
        value: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<SettingsScope, IpcError>> + Send + '_>> {
        Box::pin(async move { self.set_setting(&id, &value).await })
    }

    fn run_action(
        &self,
        id: ActionId,
        confirm: Option<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<Option<serde_json::Value>, IpcError>> + Send + '_>>
    {
        Box::pin(async move { self.run_action_with_payload(&id, confirm).await })
    }
}

fn map_fetch_error(err: &FetchError) -> ProjectedError {
    match err {
        FetchError::Classified(classified) => ProjectedError::from(classified),
        FetchError::NoAvailableStrategy(_) => ProjectedError::new(
            ProviderErrorKind::MissingCredential,
            error_kind_to_category(ProviderErrorKind::MissingCredential),
        ),
        FetchError::Cancelled => ProjectedError::new(
            ProviderErrorKind::Unknown,
            error_kind_to_category(ProviderErrorKind::Unknown),
        ),
    }
}

fn map_settings_error(err: SettingsMutationError) -> IpcError {
    match err {
        SettingsMutationError::UnknownProvider(_) | SettingsMutationError::UnknownSetting(_) => {
            IpcError::new(ErrorCode::NotFound)
        }
        SettingsMutationError::InvalidValue { .. } => IpcError::new(ErrorCode::InvalidRequest),
        SettingsMutationError::SecretNotWritableOverIpc(_) => IpcError::with_static_message(
            ErrorCode::InvalidRequest,
            "secret settings must be configured via ariadusage secret set",
        ),
    }
}

fn source_mode_label(mode: SourceMode) -> &'static str {
    match mode {
        SourceMode::Auto => "auto",
        SourceMode::Web => "web",
        SourceMode::Cli => "cli",
        SourceMode::Oauth => "oauth",
        SourceMode::Api => "api",
    }
}

/// Validates that `protocol_version` matches `PROTOCOL_V1`.
pub const fn is_supported_protocol(protocol_version: &str) -> bool {
    const_str_eq(protocol_version, PROTOCOL_V1)
}

const fn const_str_eq(lhs: &str, rhs: &str) -> bool {
    let a = lhs.as_bytes();
    let b = rhs.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn add_duration(now: Timestamp, delay: Duration) -> Timestamp {
    jiff::SignedDuration::try_from(delay)
        .ok()
        .and_then(|sd| now.checked_add(sd).ok())
        .unwrap_or(now)
}

async fn join_all_futures<'a>(futures: Vec<BoxFuture<'a, ()>>) {
    let mut pending: Vec<Option<BoxFuture<'a, ()>>> = futures.into_iter().map(Some).collect();
    std::future::poll_fn(|cx| {
        let mut all_done = true;
        for slot in &mut pending {
            if let Some(fut) = slot {
                if fut.as_mut().poll(cx).is_ready() {
                    *slot = None;
                } else {
                    all_done = false;
                }
            }
        }
        if all_done {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
}
