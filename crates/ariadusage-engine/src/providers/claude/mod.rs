pub mod account_profile;
pub mod config_paths;
pub mod credential_routing;
pub mod credentials;
pub mod identity;
pub mod plan;
pub mod planner;
pub mod rate_limit_gate;
pub mod reset_credits;
pub mod scoped_weekly;
pub mod strategy;
pub mod subscription_metadata;
pub mod usage_fetcher;
pub mod web_extras;
pub mod web_fetcher;

pub use account_profile::{ClaudeAccountProfile, ConfigOwnershipEvidence};
pub use config_paths::{
    CLAUDE_CONFIG_DIR_ENV_KEY, CLAUDE_SECURE_STORAGE_CONFIG_DIR_ENV_KEY, ClaudeConfigPaths,
};
pub use credential_routing::{ClaudeCredentialRoute, ClaudeCredentialRouting};
pub use credentials::{
    CLAUDE_OAUTH_SCOPES_ENV_KEY, CLAUDE_OAUTH_TOKEN_ENV_KEY, ClaudeOAuthCredentialRecord,
    ClaudeOAuthCredentialSource, ClaudeOAuthCredentials, ClaudeOAuthCredentialsError,
    credentials_profile_identifier, load_from_credentials_file, load_from_environment,
};
pub use identity::{ClaudeAccountIdentity, ClaudeVerifiedAccountOwner};
pub use plan::ClaudePlan;
pub use planner::{
    ClaudeFetchPlanStep, ClaudeSourcePlan, ClaudeSourcePlanner, ClaudeSourcePlanningInput,
    ClaudeSourceSelectedStrategy, ClaudeSourceSelectionReason, ClaudeUsageStrategy,
};
pub use rate_limit_gate::{ClaudeOAuthUsageRateLimitGate, parse_retry_after};
pub use reset_credits::{
    ClaudeCloudCreditsSnapshot, ClaudeRateLimitResetCredits, OAuthCedarEmberGrant,
    OAuthCedarEmberPayload, OAuthIguanaNecktiePayload,
};
pub use scoped_weekly::{ClaudeScopedWeeklyLimitMapper, OAuthScopedLimitEntry};
pub use strategy::{
    CLAUDE_ADMIN_API_STRATEGY_ID, CLAUDE_CLI_STRATEGY_ID, CLAUDE_OAUTH_STRATEGY_ID,
    CLAUDE_WEB_STRATEGY_ID, ClaudeOAuthFetchStrategy, ClaudeOAuthStrategyConfig,
    ClaudeProviderPlan, ClaudeUnavailableFetchStrategy, ClaudeWebAvailabilityProbe,
    ClaudeWebEnrichmentAccess, ClaudeWebFetchDeadlineState, ClaudeWebFetchStrategy,
    ClaudeWebFetchStrategyError, ClaudeWebUsageLoader, DEFAULT_CLAUDE_WEB_TIMEOUT_SECONDS,
};
pub use subscription_metadata::{
    BillingDate, ClaudeSubscriptionFetchResult, ClaudeSubscriptionMetadata,
    ClaudeSubscriptionMetadataFetcher, ClaudeSubscriptionParseError,
    DEFAULT_SUBSCRIPTION_METADATA_TIMEOUT,
};
pub use usage_fetcher::{
    BETA_HEADER_KEY, BETA_HEADER_VALUE, ClaudeOAuthEndpointConfig, ClaudeOAuthFetchError,
    ClaudeOAuthMappedUsage, ClaudeOAuthUsageFetcher, DEFAULT_BASE_URL, DEFAULT_CLAUDE_CLI_VERSION,
    OAuthExtraUsage, OAuthUsageResponse, OAuthUsageWindow, PROFILE_PATH, USAGE_PATH,
};
pub use web_extras::{
    WebExtrasApplyParams, apply_web_extras_if_needed, merge_extra_rate_windows,
    merge_provider_cost, replacing_web_extras, web_extras_account_matches,
};
pub use web_fetcher::{
    CLOUDFLARE_CHALLENGE_MESSAGE, ClaudeWebAPIFetcher, ClaudeWebBrowserFetchGate,
    ClaudeWebEndpointConfig, ClaudeWebExtraRateWindowParser, ClaudeWebFetchError,
    ClaudeWebFetchOptions, ClaudeWebSessionImporter, ClaudeWebSessionKeyRenewalTracker,
    DEFAULT_PREPAID_CREDITS_TIMEOUT, DEFAULT_WEB_BASE_URL, OrganizationInfo, PrepaidBalance,
    SESSION_KEY_COOKIE_NAME, SESSION_KEY_PREFIX, SessionKeyInfo, WebAccountInfo, WebUsageData,
    claude_declared_domains,
};
