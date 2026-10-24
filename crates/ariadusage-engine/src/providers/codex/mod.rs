// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

pub mod cli_backend_config;
pub mod cli_launch_gate;
pub mod cli_rpc;
pub mod credentials;
pub mod credits;
pub mod dashboard_authority;
pub mod identity;
pub mod normalizer;
pub mod openai_dashboard_cookie_importer;
pub mod openai_dashboard_fetcher;
pub mod openai_dashboard_models;
pub mod openai_dashboard_parser;
pub mod pat;
pub mod plan;
pub mod reconciled;
pub mod strategy;
pub mod usage_fetcher;

pub use cli_backend_config::{CodexCLIBackend, CodexCLIBackendRateLimitError, CodexUIErrorMapper};
pub use cli_launch_gate::CodexCLILaunchGate;
pub use cli_rpc::{
    AccountInfo, CodexCLIAccountSnapshot, CodexCLIFetchError, CodexCLIUsageFetcher,
    CodexStatusProbeError, CodexStatusSnapshot, DEFAULT_CODEX_APP_SERVER_ARGS, RPCAccountDetails,
    RPCAccountResponse, RPCCreditsSnapshot, RPCRateLimitSnapshot, RPCRateLimitWindow,
    RPCRateLimitsResponse, RPCSpendControlLimitSnapshot, RPCWireError, UsageError,
    UsageLimitsAvailability,
};
pub use credentials::{
    CodexHomeScope, CodexOAuthCredentialSource, CodexOAuthCredentials, CodexOAuthCredentialsError,
    CodexOAuthCredentialsStore, CodexPATCredentials, CodexRefreshError, CodexTokenRefresher,
    parse_jwt_payload,
};
pub use credits::{
    CodexCreditLimitSnapshot, CodexExtraUsageCost, CodexRateLimitResetCredit,
    CodexRateLimitResetCreditInventory, CodexRateLimitResetCreditStatus,
    CodexRateLimitResetCreditsSnapshot, CreditEvent, CreditsSnapshot,
};
pub use dashboard_authority::{
    CodexCLIDashboardAuthorityContext, CodexDashboardAllowedEffect, CodexDashboardAuthority,
    CodexDashboardAuthorityDecision, CodexDashboardAuthorityInput, CodexDashboardCleanup,
    CodexDashboardDecisionReason, CodexDashboardDisposition, CodexDashboardKnownOwnerCandidate,
    CodexDashboardOwnershipProofContext, CodexDashboardPolicyError, CodexDashboardRoutingHints,
    CodexDashboardSourceKind, CodexOpenAIWorkspaceIdentity, CodexOpenAIWorkspaceIdentityCache,
    CodexOpenAIWorkspaceResolver, CodexOpenAIWorkspaceResolverError, DEFAULT_CHATGPT_ACCOUNTS_URL,
};
pub use identity::{
    CodexAuthBackedAccount, CodexIdentity, CodexIdentityResolver,
    CodexNativeCredentialOwnerIdentity,
};
pub use normalizer::{
    AdditionalRateLimit, AdditionalRateLimitDetails, CodexAdditionalRateLimitMapper,
    CodexRateWindowNormalizer, WindowSnapshot,
};
pub use openai_dashboard_cookie_importer::{
    FoundAccount, OPENAI_COOKIE_DOMAINS, OpenAIDashboardBrowserCookieImporter,
    OpenAIDashboardCandidateSource, OpenAIDashboardCookieCandidate,
    OpenAIDashboardCookieImportError, OpenAIDashboardCookieImportResult,
    OpenAIDashboardPageScrapeGate, OpenAIWebRefreshGateContext, OpenAIWebRefreshPolicyContext,
    PersistentCookieSessionError, actionable_openai_dashboard_import_failure,
    concise_openai_cookie_mismatch_status, cookie_summary_from_header, dashboard_has_page_history,
    force_openai_web_refresh_for_stale_request, has_session_cookies_in_header,
    is_openai_session_cookie_name, openai_dashboard_friendly_error, openai_declared_domains,
    openai_web_refresh_phase, openai_web_target_isolation_key, refresh_phase,
    should_allow_openai_dashboard_page_scrape, should_attempt_cookie_import,
    should_run_openai_web_refresh, should_skip_openai_web_empty_history_retry,
    should_skip_openai_web_refresh,
};
pub use openai_dashboard_fetcher::{
    CHATGPT_HOST, DEFAULT_CHATGPT_WEB_BASE_URL, DEFAULT_CREDITS_PURCHASE_URL, DashboardAPIData,
    DashboardAPIResponse, DashboardScrapePayload, OpenAIDashboardEndpointConfig,
    OpenAIDashboardFetchError, OpenAIDashboardFetcher, SessionAuthorization,
};
pub use openai_dashboard_models::{
    DEFAULT_OPENAI_SUBSCRIPTIONS_URL, OpenAIDashboardCache, OpenAIDashboardCacheStore,
    OpenAIDashboardDailyBreakdown, OpenAIDashboardServiceUsage, OpenAIDashboardSnapshot,
    OpenAIDashboardUsageBreakdownSummary, OpenAISubscriptionFetchResult,
    OpenAISubscriptionMetadata, OpenAISubscriptionMetadataParser,
};
pub use openai_dashboard_parser::OpenAIDashboardParser;
pub use pat::{
    CodexCLIUserAgent, CodexPATUsageFetch, CodexPATUsageFetcher, DEFAULT_PAT_WHOAMI_URL,
    PAT_WHOAMI_PATH,
};
pub use plan::CodexPlanFormatting;
pub use reconciled::{
    CodexPATWhoami, CodexReconciledState, CodexUsageLane, classify_codex_lane, icon_windows,
    plan_utilization_series, visible_windows, weekly_caps_session, windows_by_lane,
};
pub use strategy::{
    CODEX_CLI_STRATEGY_ID, CODEX_OAUTH_NATIVE_REFRESH_CLI_STRATEGY_ID, CODEX_OAUTH_STRATEGY_ID,
    CODEX_PAT_STRATEGY_ID, CODEX_WEB_DASHBOARD_STRATEGY_ID, CodexCLIUsageStrategy,
    CodexFetchOutcome, CodexOAuthError, CodexOAuthFetchStrategy,
    CodexOAuthNativeRefreshCLIStrategy, CodexPATFetchStrategy, CodexProviderPlan,
    CodexStrategyConfig, CodexUsageDataSource, CodexUsageStrategy, CodexWebDashboardError,
    CodexWebDashboardStrategy, OpenAIWebCodexError, OpenAIWebCodexResult, no_data_message,
    resolve_usage_strategy,
};
pub use usage_fetcher::{
    CHATGPT_USAGE_PATH, CODEX_USAGE_PATH, CodexOAuthFetchError, CodexOAuthUsageFetcher,
    CodexSpendControlNumber, CodexSpendControlsMonthlyUsageGate,
    CodexSpendControlsMonthlyUsageResponse, CodexUsageResponse,
    CodexWorkspaceRemainingBalanceResponse, CreditDetails, DEFAULT_CHATGPT_BASE_URL,
    EffectiveMonthlyLimit, PlanType, RATE_LIMIT_RESET_CREDITS_PATH, RateLimitDetails,
    SPEND_CONTROLS_MONTHLY_USAGE_PATH_SUFFIX, SpendControlLimitSnapshot,
    WORKSPACE_REMAINING_BALANCE_PATH_SUFFIX,
};
