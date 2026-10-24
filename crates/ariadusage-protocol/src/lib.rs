//! Protocol types, schemas, and framing for AriadUsage.

pub mod cli_cost_config;
pub mod ids;
pub mod ipc;
pub mod metric;
pub mod secret;
pub mod settings;
pub mod snapshot;
pub mod time;
pub mod usage;

#[cfg(feature = "codec")]
pub mod codec;

#[cfg(all(feature = "fixture", unix))]
pub mod fixture;

pub use cli_cost_config::{
    ConfigProviderStatusResult, ConfigProviderToggleResult, ConfigSetSourceResult,
    CostDailyEntryPayload, CostErrorPayload, CostModelBreakdownPayload, CostPayload,
    CostProjectPayload, CostProjectSourcePayload, CostTotalsPayload, CostUsageCoverageCounts,
};
pub use ids::{ActionId, IdError, ProviderId, RequestId, SettingId};
pub use ipc::{
    ClientInfo, ClientMessage, ErrorCode, IpcError, IpcMessage, MAX_FRAME_BYTES, NoticeLevel,
    PROTOCOL_V1, RawEnvelope, SUPPORTED_PROTOCOLS, ServerMessage, Topic, negotiate_protocol,
    parse_client_message, parse_server_message,
};
pub use metric::{
    Confidence, Metric, MetricError, MetricInvariantError, MetricSource, MetricState, SourceKind,
};
pub use secret::SecretString;
pub use settings::{
    ActionConfirmation, ActionItem, ActionStyle, ChoiceOption, DescriptorKind, MultiChoiceEntry,
    NumberConfig, SettingCondition, SettingDescriptor, SettingsPage, SettingsScope,
    SettingsSection, TextConfig, TokenAccountRow, TokenAccountsConfig,
};
pub use snapshot::{
    AccountSnapshot, EngineInfo, EngineSnapshot, ProviderPresentation, ProviderPresentationError,
    ProviderSnapshot, ProviderWindows,
};
pub use usage::{
    BillingDate, Chart, ChartKind, ChartPoint, Cost, CostDailyEntry, CostHistory,
    CostModelBreakdown, CostProvenance, CostWindowSummary, CreditLimit, Credits, DetailRow,
    DetailSection, DetailSectionValidationError, Identity, MAXIMUM_POINTS_PER_CHART,
    MAXIMUM_ROWS_PER_SECTION, MAXIMUM_SECTIONS_PER_SNAPSHOT, MAXIMUM_STRING_LENGTH, NamedWindow,
    Pace, PaceStage, PlanUtilizationEntry, PlanUtilizationSeries, ProviderError,
    ProviderErrorCategory, ProviderErrorKind, ProviderStatus, RateWindow, ResetCredits,
    RowProgress, RowSeverity, SessionEquivalentForecast, StatusComponent, StatusIndicator,
    SubscriptionDates,
};

#[cfg(feature = "codec")]
pub use codec::{IpcCodec, IpcCodecError};

#[cfg(all(feature = "fixture", unix))]
pub use fixture::{
    FixtureConfig, FixtureServerHandle, MisbehaveMode, ensure_socket_directory,
    generate_scenario_snapshot, generate_scenario_snapshot_with_extra, start_fixture_server,
};
