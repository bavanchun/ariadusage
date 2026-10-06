//! Protocol types, schemas, and framing for AriadUsage.

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

pub use ids::{ActionId, IdError, ProviderId, RequestId, SettingId};
pub use ipc::{
    negotiate_protocol, parse_client_message, parse_server_message, ClientInfo, ClientMessage,
    ErrorCode, IpcError, NoticeLevel, RawEnvelope, ServerMessage, Topic, PROTOCOL_V1,
    SUPPORTED_PROTOCOLS,
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
pub use snapshot::{AccountSnapshot, EngineInfo, EngineSnapshot, ProviderSnapshot, ProviderWindows};
pub use usage::{
    Chart, ChartKind, ChartPoint, Cost, Credits, DetailRow, DetailSection,
    DetailSectionValidationError, Identity, NamedWindow, Pace, PaceStage, ProviderError,
    ProviderErrorCategory, ProviderErrorKind, RateWindow, RowProgress, StatusIndicator,
    MAXIMUM_POINTS_PER_CHART, MAXIMUM_ROWS_PER_SECTION, MAXIMUM_SECTIONS_PER_SNAPSHOT,
    MAXIMUM_STRING_LENGTH,
};

#[cfg(feature = "codec")]
pub use codec::{IpcCodec, IpcCodecError, MAX_FRAME_BYTES};
