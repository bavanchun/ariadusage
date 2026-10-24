pub mod brokers;
pub mod config_store;
pub mod coordinator;
pub mod cost;
pub mod daemon;
pub mod error;
pub mod hardening;
pub mod hooks;
pub mod host_observers;
pub mod ipc_server;
pub mod paths;
pub mod private_file;
pub mod private_tempdir;
pub mod providers;
pub mod registry;
pub mod runtime;
pub mod settings_page;
pub mod state_store;
pub mod status;
pub mod trust;

pub use config_store::{ConfigStore, UpdateResult};
pub use coordinator::{
    ProviderRefreshCoordinator, ProviderRefreshRequestState, Replacement, WaitResult,
};
pub use daemon::{
    CompositeNotifier, ConfigIpcBackend, DaemonError, DefaultNotifier, IpcNoticeNotifier,
    LinuxDbusNotifier, NoopNotifier, NotificationError, Notifier, RecordingNotifier,
    build_default_secret_store,
};
pub use error::StoreError;
pub use hooks::{
    DispatchOutcome, FORWARDED_ENVIRONMENT_KEYS, FakeHookCommandExecutor, HookCommandExecutor,
    HookExecutionOutput, HookExecutionRequest, HookRunner, HookRunnerError, MAXIMUM_PAYLOAD_BYTES,
    ProcessBrokerHookExecutor,
};
pub use host_observers::{
    AgentProcessSnapshot, AgentSessionProvider, AgentSessionRefreshGate,
    ClaudeDesktopProjectsLocator, ClaudeSessionProjectMapper, DefaultHostPowerThermalProbe,
    DirectoryMetadataScanBudget, FutureModificationDateClamp, HostPowerThermalProbe,
    LOCAL_ACTIVITY_SCAN_INTERVAL, LocalActivityScanResult, LocalAgentSessionScanner,
    ObservedAgentSession, SessionScanConfig, evaluate_upower_low_power_mode,
    map_macos_thermal_state, read_linux_thermal_pressure_from_sysfs,
};
pub use ipc_server::{IpcBackend, IpcServerError, PeerIdentity, verify_peer_uid};
pub use paths::{
    ENGINE_SOCKET_REL_PATH, PathError, SUN_PATH_MAX_BYTES, canonicalize_macos_var_prefix,
    config_path, data_dir, resolve_config_path, resolve_data_dir, resolve_macos_runtime_dir_with,
    resolve_runtime_dir, resolve_socket_path_from_runtime_dir, resolve_state_dir, runtime_dir,
    socket_path, state_dir, trusted_runtime_dir, validate_socket_path_len,
};
pub use private_file::{WriteHooks, repair_permissions, write_private};
pub use private_tempdir::{PrivateTempDir, TempDirError};
pub use registry::{
    ClaudeProviderPlanFactory, CodexProviderPlanFactory, ProviderPlanFactory, ProviderPlanInputs,
    ProviderRegistry, ResolvedProviderSpec, UnregisteredProviderPlan,
};
pub use runtime::{
    DEFAULT_STALE_AFTER_SECONDS, EngineRuntime, NoopTransitionSink, RecordingTransitionSink,
    RefreshOptions, SnapshotStore, TokioSleeper, TransitionSink, is_supported_protocol,
};
pub use settings_page::{
    AppPreferences, SettingsMutationError, apply_setting_mutation, build_settings_page,
};
pub use state_store::{BrokerState, BrokerStateStore, StateWarning};
pub use status::{
    DEFAULT_STATUS_TIMEOUT, GOOGLE_WORKSPACE_INCIDENTS_URL, ProviderStatusFetcher,
    ProviderStatusSummary, StatusFetchError, StatusParseError, StatuspageComponentSummary,
    status_declared_hosts, status_declared_origins,
};
pub use trust::TrustPolicy;
