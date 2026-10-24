// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

pub mod cli_https;
pub mod cli_print_failure;
pub mod cli_session;
pub mod descriptor;
pub mod family_visibility;
pub mod jsonl_object;
pub mod local_jsonl;
pub mod local_reader;
pub mod local_scan;
pub mod local_sqlite;
pub mod local_sqlite_schema;
pub mod oauth_store;
pub mod offline_store;
pub mod proto_reader;
pub mod quota_summary;
pub mod remote_fetcher;
pub mod scoped_print_fetch;
pub mod status_probe;

pub use descriptor::{
    ANTIGRAVITY_APP_LOCAL_STRATEGY_ID, ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
    ANTIGRAVITY_IDE_LOCAL_STRATEGY_ID, ANTIGRAVITY_OAUTH_STRATEGY_ID,
    ANTIGRAVITY_OFFLINE_STRATEGY_ID, AntigravityOAuthFetchStrategy,
    AntigravityOfflineFetchStrategy, AntigravityOfflinePriorError, AntigravityProviderPlan,
    AntigravityProviderPresentation, AntigravitySelectedAccountGuard,
    AntigravityStatusFetchStrategy, AntigravityStatusProbeOverride, AntigravityStrategyConfig,
    AntigravityUsageDataSource, IconWindowsContext, IconWindowsPair, MenuBarMetric,
    MenuBarWindowContext, MenuBarWindowSelection, SemanticWindows,
};
pub use family_visibility::{
    AntigravityQuotaFamilyVisibility, KnownFamily, idle_window_ids, known_family,
    known_family_for_window,
};
pub use jsonl_object::{AntigravityJSONLNumber, AntigravityJSONLObject, AntigravityJSONLValue};
pub use local_reader::{
    AntigravityLocalReader, CostProvenance, CostUsageCoverageCounts, CostUsageDailyReport,
    CostUsageDailyReportEntry, CostUsageDailyReportSummary, CostUsageModelBreakdown,
    CostUsageTokenMix, CostUsageTokenSnapshot, CostUsageWindowSummary, Coverage, DailyReportResult,
    Event, ModelsDevCache, ModelsDevCatalog, ModelsDevCost, ModelsDevModel, ModelsDevProvider,
    SourceResult,
};
pub use local_scan::{Budget, Context, Discovery, Limits, ScanFailure, Statistics};
pub use local_sqlite_schema::{SQLitePayload, SQLiteTableSupport};
pub use oauth_store::{
    AntigravityOAuthClient, AntigravityOAuthConfig, AntigravityOAuthCredentials,
    AntigravityOAuthCredentialsStore,
};
pub use offline_store::AntigravityOfflineStore;
pub use proto_reader::{
    AntigravityProtoReader, AuxiliaryIdentifier, CheckedSum, ParsedTurn, ParsedUsage, ProtoField,
    StepMetadata,
};
pub use quota_summary::{
    AntigravityQuotaSummary, AntigravityQuotaSummaryBucket, AntigravityQuotaSummaryGroup,
    parse_cli_usage_report, parse_quota_summary_response, parse_quota_summary_response_with_source,
};
pub use remote_fetcher::{
    ANTIGRAVITY_GOOGLE_ORIGINS, AllowedTier, AntigravityRemoteFetchError,
    AntigravityRemoteUsageFetcher, CloudAiCompanionProject, CredentialsUpdateHandler,
    FETCH_AVAILABLE_MODELS_ENDPOINT, IneligibleTier, LOAD_CODE_ASSIST_ENDPOINT,
    LoadCodeAssistResponse, OAuthClientResolver, ONBOARD_USER_ENDPOINT, PlanInfo,
    RETRIEVE_USER_QUOTA_ENDPOINT, RETRIEVE_USER_QUOTA_SUMMARY_ENDPOINT, RemoteDataLoader,
    RemoteHttpRequest, RemoteHttpResponse, TOKEN_REFRESH_ENDPOINT, Tier,
};
pub use status_probe::{
    AntigravityConnectionEndpoint, AntigravityModelQuota, AntigravityPlanInfoSummary,
    AntigravityProcessKind, AntigravitySnapshotSource, AntigravityStatusProbe,
    AntigravityStatusProbeError, AntigravityStatusSnapshot, AntigravityUsagePool,
    COMMAND_MODEL_CONFIG_PATH, COMPACT_FALLBACK_WINDOW_ID_PREFIX, CURRENT_FLASH_CANONICAL_ID,
    EndpointRequestSender, EndpointSource, GET_USER_STATUS_PATH, LocalhostTrustPolicy,
    OperationDeadline, ProcessEntry, ProcessInfoResult, ProcessScope, QUOTA_SUMMARY_PATH,
    QUOTA_SUMMARY_WINDOW_ID_PREFIX, RequestContext, RequestPayload, ResponseSummary, UNLEASH_PATH,
};

pub use cli_https::*;
pub use cli_print_failure::{AntigravityCLIPrintFailure, ExitReason};
pub use cli_session::*;
pub use scoped_print_fetch::*;
