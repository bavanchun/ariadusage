// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexUsageDataSource.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexPAT/CodexPATFetchStrategy.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::model::identity::ProviderIdentitySnapshot;
use ariadusage_core::model::snapshot::UsageSnapshot;
use ariadusage_core::pipeline::{
    BoxFuture, ClassifiedError, FetchContext, FetchError, FetchResult, FetchRuntime, FetchStrategy,
    ProviderPlan,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_protocol::metric::{Metric, MetricSource, SourceKind};
use ariadusage_protocol::snapshot::{ProviderPresentation, ProviderSnapshot, ProviderWindows};
use ariadusage_protocol::usage::{
    BillingDate, Cost, Credits, Identity, NamedWindow, ProviderErrorKind,
    RateWindow as WireRateWindow, SubscriptionDates,
};
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::brokers::call::BrokerCall;
use crate::brokers::net::{NetBroker, NetConfig};

use super::cli_rpc::{CodexCLIFetchError, CodexCLIUsageFetcher, UsageError};
use super::credentials::{
    CodexHomeScope, CodexOAuthCredentialSource, CodexOAuthCredentials, CodexOAuthCredentialsError,
    CodexOAuthCredentialsStore, CodexRefreshError, CredentialReadFn,
};
use super::credits::{CodexExtraUsageCost, CodexRateLimitResetCreditsSnapshot, CreditsSnapshot};
use super::pat::CodexPATUsageFetcher;
use super::plan::CodexPlanFormatting;
use super::reconciled::{CodexPATWhoami, CodexReconciledState};
use super::usage_fetcher::{
    CodexOAuthFetchError, CodexOAuthUsageFetcher, CodexSpendControlsMonthlyUsageGate,
    CodexSpendControlsMonthlyUsageResponse, CodexUsageResponse,
    CodexWorkspaceRemainingBalanceResponse,
};

pub const CODEX_PAT_STRATEGY_ID: &str = "codex.pat";
pub const CODEX_OAUTH_STRATEGY_ID: &str = "codex.oauth";
pub const CODEX_OAUTH_NATIVE_REFRESH_CLI_STRATEGY_ID: &str = "codex.oauth-native-refresh-cli";
pub const CODEX_CLI_STRATEGY_ID: &str = "codex.cli";
pub const CODEX_WEB_DASHBOARD_STRATEGY_ID: &str = "codex.web.dashboard";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodexUsageDataSource {
    Auto,
    Pat,
    OAuth,
    Cli,
}

impl CodexUsageDataSource {
    pub fn id(self) -> &'static str {
        self.source_label()
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Pat => "PAT",
            Self::OAuth => "OAuth API",
            Self::Cli => "CLI (RPC/PTY)",
        }
    }

    pub fn source_label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Pat => "pat",
            Self::OAuth => "oauth",
            Self::Cli => "cli",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexUsageStrategy {
    pub data_source: CodexUsageDataSource,
}

pub fn resolve_usage_strategy(
    selected_data_source: CodexUsageDataSource,
    has_oauth_credentials: bool,
    has_pat_credentials: bool,
) -> CodexUsageStrategy {
    if selected_data_source == CodexUsageDataSource::Auto {
        if has_pat_credentials {
            return CodexUsageStrategy {
                data_source: CodexUsageDataSource::Pat,
            };
        }
        if has_oauth_credentials {
            return CodexUsageStrategy {
                data_source: CodexUsageDataSource::OAuth,
            };
        }
        return CodexUsageStrategy {
            data_source: CodexUsageDataSource::Cli,
        };
    }
    CodexUsageStrategy {
        data_source: selected_data_source,
    }
}

pub fn no_data_message(env: &BTreeMap<String, String>, home_dir: Option<&Path>) -> String {
    let base =
        CodexHomeScope::ambient_home_url(env, home_dir).unwrap_or_else(|| PathBuf::from(".codex"));
    let base_str = base.to_string_lossy();
    format!("No Codex sessions found in {base_str}/sessions or {base_str}/archived_sessions.")
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexFetchOutcome {
    pub usage: UsageSnapshot,
    pub credits: Option<CreditsSnapshot>,
    pub codex_reset_credits: Option<CodexRateLimitResetCreditsSnapshot>,
    pub source_label: String,
    pub strategy_id: &'static str,
    pub strategy_kind: SourceKind,
    pub codex_reset_credits_attempted: bool,
    pub codex_monthly_limit_enrichment_failed: bool,
}

impl CodexFetchOutcome {
    pub fn marking_monthly_limit_enrichment_failed(mut self) -> Self {
        self.codex_monthly_limit_enrichment_failed = true;
        self
    }

    pub fn into_fetch_result(self) -> FetchResult {
        FetchResult::new(
            self.usage,
            self.source_label,
            self.strategy_id,
            self.strategy_kind,
        )
    }

    pub fn project_provider_snapshot(&self, now: Timestamp) -> ProviderSnapshot {
        let codex_id = ProviderId::new("codex").expect("valid provider id");
        let mut snap =
            ProviderSnapshot::new(codex_id.clone(), "Codex", true, self.source_label.clone());
        snap.presentation = ProviderPresentation::new(
            "Codex",
            "Session",
            "Weekly",
            None,
            Some(ProviderPresentation::accent_color_from_rgb(73, 163, 176)),
        )
        .ok();
        let src = MetricSource::new(self.strategy_kind, self.source_label.clone());
        let conf = self.usage.data_confidence;
        let as_of = Some(self.usage.updated_at);

        let map_win = |w: &ariadusage_core::model::window::RateWindow| -> WireRateWindow {
            let mut wire = WireRateWindow::new(w.used_percent);
            wire.window_minutes = w.window_minutes.and_then(|m| u32::try_from(m).ok());
            wire.resets_at = w.resets_at;
            wire.reset_description = w.reset_description.clone();
            wire.next_regen_percent = w.next_regen_percent;
            wire
        };

        snap.windows = ProviderWindows {
            primary: self
                .usage
                .primary
                .as_ref()
                .map(|w| Metric::value(map_win(w), as_of, Some(src.clone()), conf)),
            secondary: self
                .usage
                .secondary
                .as_ref()
                .map(|w| Metric::value(map_win(w), as_of, Some(src.clone()), conf)),
            tertiary: self
                .usage
                .tertiary
                .as_ref()
                .map(|w| Metric::value(map_win(w), as_of, Some(src.clone()), conf)),
            extra: self
                .usage
                .extra_rate_windows
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|nw| NamedWindow {
                    id: nw.id.clone(),
                    title: nw.title.clone(),
                    window: Metric::value(map_win(&nw.window), as_of, Some(src.clone()), conf),
                })
                .collect(),
        };

        let reset_wire = self
            .codex_reset_credits
            .as_ref()
            .and_then(|rc| rc.to_protocol_reset_credits(now));

        if let Some(credits) = &self.credits {
            snap.credits = Some(Metric::value(
                credits.to_protocol_credits(reset_wire),
                Some(credits.updated_at),
                Some(src.clone()),
                conf,
            ));
        } else if let Some(reset_wire) = reset_wire {
            let mut c = Credits::new(0.0);
            c.balance_read_succeeded = Some(false);
            c.reset_credits = Some(reset_wire);
            snap.credits = Some(Metric::value(c, as_of, Some(src.clone()), conf));
        }

        if let Some(cost) = &self.usage.provider_cost {
            snap.cost = Some(Metric::value(
                Cost {
                    used: cost.used,
                    limit: (cost.limit > 0.0).then_some(cost.limit),
                    currency_code: cost.currency_code.clone(),
                    period: cost.period.clone(),
                    resets_at: cost.resets_at,
                    balance: cost.balance,
                    next_regen_amount: cost.next_regen_amount,
                    personal_used: cost.personal_used,
                    balance_updated_at: cost.balance_updated_at,
                    balance_is_workspace: cost.balance_is_workspace,
                    balance_is_unavailable: cost.balance_is_unavailable,
                },
                Some(cost.updated_at),
                Some(src.clone()),
                conf,
            ));
        }

        if let Some(id) = &self.usage.identity {
            let plan = id
                .login_method
                .as_deref()
                .and_then(CodexPlanFormatting::display_name)
                .or_else(|| id.login_method.clone());
            snap.identity = Some(Metric::value(
                Identity {
                    provider_id: id.provider_id.clone().unwrap_or(codex_id),
                    account_email: id.account_email.clone(),
                    account_organization: id.account_organization.clone(),
                    login_method: id.login_method.clone(),
                    account_id: None,
                    plan,
                },
                as_of,
                Some(src.clone()),
                conf,
            ));
        }

        if self.usage.subscription_renews_at.is_some()
            || self.usage.subscription_expires_at.is_some()
        {
            snap.subscription = Some(Metric::value(
                SubscriptionDates {
                    renews: self.usage.subscription_renews_at.map(|at| BillingDate {
                        at,
                        date_only: self.usage.subscription_renews_at_is_date_only,
                    }),
                    expires: self.usage.subscription_expires_at.map(|at| BillingDate {
                        at,
                        date_only: self.usage.subscription_expires_at_is_date_only,
                    }),
                },
                as_of,
                Some(src),
                conf,
            ));
        }

        snap.updated_at = Some(self.usage.updated_at);
        snap
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexOAuthError {
    Fetch(CodexOAuthFetchError),
    Credentials(CodexOAuthCredentialsError),
    Refresh(CodexRefreshError),
    NoRateLimitsFound,
}

impl From<CodexOAuthFetchError> for CodexOAuthError {
    fn from(value: CodexOAuthFetchError) -> Self {
        Self::Fetch(value)
    }
}

impl From<CodexOAuthCredentialsError> for CodexOAuthError {
    fn from(value: CodexOAuthCredentialsError) -> Self {
        Self::Credentials(value)
    }
}

impl From<CodexRefreshError> for CodexOAuthError {
    fn from(value: CodexRefreshError) -> Self {
        Self::Refresh(value)
    }
}

impl CodexOAuthError {
    pub fn into_fetch_error(self) -> FetchError {
        match self {
            Self::Fetch(e) => e.into_fetch_error(),
            Self::Credentials(e) => {
                let kind = match e {
                    CodexOAuthCredentialsError::Cancelled => return FetchError::Cancelled,
                    CodexOAuthCredentialsError::NotFound
                    | CodexOAuthCredentialsError::MissingTokens => {
                        ProviderErrorKind::MissingCredential
                    }
                    CodexOAuthCredentialsError::Unreadable
                    | CodexOAuthCredentialsError::WriteFailed => {
                        ProviderErrorKind::PermissionDenied
                    }
                    CodexOAuthCredentialsError::DecodeFailed(_) => ProviderErrorKind::ParseFailure,
                    CodexOAuthCredentialsError::ReadOnlySource
                    | CodexOAuthCredentialsError::NativeRefreshRequired => {
                        ProviderErrorKind::AuthenticationExpired
                    }
                };
                FetchError::Classified(ClassifiedError::new(kind, e.to_string()))
            }
            Self::Refresh(e) => {
                let kind = match e {
                    CodexRefreshError::Cancelled => return FetchError::Cancelled,
                    CodexRefreshError::Expired
                    | CodexRefreshError::Revoked
                    | CodexRefreshError::Reused => ProviderErrorKind::AuthenticationExpired,
                    CodexRefreshError::NetworkError(_) => ProviderErrorKind::NetworkFailure,
                    CodexRefreshError::InvalidResponse(_) => ProviderErrorKind::ParseFailure,
                };
                FetchError::Classified(ClassifiedError::new(kind, e.to_string()))
            }
            Self::NoRateLimitsFound => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::ParseFailure,
                UsageError::NoRateLimitsFound.to_string(),
            )),
        }
    }
}

#[derive(Clone)]
pub struct CodexStrategyConfig {
    pub net: Arc<NetBroker>,
    pub environment: BTreeMap<String, String>,
    pub home_directory: PathBuf,
    pub working_directory: PathBuf,
    pub managed_workspace_account_id: Option<String>,
    pub allow_external_oauth_sources: bool,
    pub resolved_cli_version: Option<String>,
    pub clock: Arc<dyn Fn() -> Timestamp + Send + Sync>,
    pub cli_fetcher: CodexCLIUsageFetcher,
}

impl CodexStrategyConfig {
    pub fn new(
        net: Arc<NetBroker>,
        environment: BTreeMap<String, String>,
        home_directory: impl Into<PathBuf>,
        working_directory: impl Into<PathBuf>,
    ) -> Self {
        let home_directory = home_directory.into();
        let working_directory = working_directory.into();
        let cli_fetcher = CodexCLIUsageFetcher::new(
            environment.clone(),
            Some(home_directory.clone()),
            working_directory.clone(),
        );
        Self {
            net,
            environment,
            home_directory,
            working_directory,
            managed_workspace_account_id: None,
            allow_external_oauth_sources: false,
            resolved_cli_version: None,
            clock: Arc::new(Timestamp::now),
            cli_fetcher,
        }
    }

    pub fn with_resolved_cli_version(mut self, version: Option<String>) -> Self {
        self.resolved_cli_version = version;
        self
    }
}

#[derive(Clone)]
pub struct CodexOAuthFetchStrategy {
    pub config: CodexStrategyConfig,
}

impl CodexOAuthFetchStrategy {
    pub fn new(config: CodexStrategyConfig) -> Self {
        Self { config }
    }

    pub async fn load_credentials(
        &self,
        retry_stale: bool,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        self.load_credentials_with_call(retry_stale, None).await
    }

    pub async fn load_credentials_with_call(
        &self,
        retry_stale: bool,
        call: Option<&BrokerCall>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        let now = (self.config.clock)();
        let env = self.config.environment.clone();
        let home = self.config.home_directory.clone();
        let allow_external = self.config.allow_external_oauth_sources;
        let call_owned = call.cloned();
        Self::load_credentials_with_reader_and_call(retry_stale, now, call, move || {
            CodexOAuthCredentialsStore::load_for_usage_with_reader_and_call(
                &env,
                allow_external,
                Some(&home),
                None,
                call_owned.as_ref(),
            )
        })
        .await
    }

    pub async fn load_credentials_with_reader<F>(
        retry_stale: bool,
        now: Timestamp,
        reader: F,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError>
    where
        F: FnMut() -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError>,
    {
        Self::load_credentials_with_reader_and_call(retry_stale, now, None, reader).await
    }

    pub async fn load_credentials_with_reader_and_call<F>(
        retry_stale: bool,
        now: Timestamp,
        call: Option<&BrokerCall>,
        mut reader: F,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError>
    where
        F: FnMut() -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError>,
    {
        let mut retries_remaining = 2usize;
        loop {
            if call.is_some_and(|c| c.cancel.is_cancelled()) {
                return Err(CodexOAuthCredentialsError::Cancelled);
            }
            match reader() {
                Ok(credentials) => {
                    if !retry_stale
                        || credentials.source != CodexOAuthCredentialSource::CodexHome
                        || !credentials.needs_refresh_at(now)
                        || retries_remaining == 0
                    {
                        return Ok(credentials);
                    }
                }
                Err(CodexOAuthCredentialsError::Cancelled) => {
                    return Err(CodexOAuthCredentialsError::Cancelled);
                }
                Err(err) => {
                    if retries_remaining == 0 {
                        return Err(err);
                    }
                }
            }
            retries_remaining -= 1;
            if let Some(c) = call {
                tokio::select! {
                    () = c.cancel.cancelled() => return Err(CodexOAuthCredentialsError::Cancelled),
                    () = tokio::time::sleep(Duration::from_millis(50)) => {}
                }
            } else {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    pub fn prepare_credentials_for_usage(
        credentials: &CodexOAuthCredentials,
        now: Timestamp,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        if !credentials.needs_refresh_at(now) {
            return Ok(credentials.clone());
        }
        Err(
            if credentials.source == CodexOAuthCredentialSource::CodexHome {
                CodexOAuthCredentialsError::NativeRefreshRequired
            } else {
                CodexOAuthCredentialsError::ReadOnlySource
            },
        )
    }

    pub fn should_fetch_reset_credits(ctx: &FetchContext) -> bool {
        match ctx.runtime {
            FetchRuntime::Daemon => true,
            FetchRuntime::Cli => ctx.include_credits,
        }
    }

    pub fn should_fallback_on_error(error: &CodexOAuthError, source_mode: SourceMode) -> bool {
        let is_explicit_native_refresh = matches!(
            error,
            CodexOAuthError::Credentials(CodexOAuthCredentialsError::NativeRefreshRequired)
        );
        if source_mode != SourceMode::Auto
            && !(source_mode == SourceMode::Oauth && is_explicit_native_refresh)
        {
            return false;
        }
        match error {
            CodexOAuthError::Fetch(CodexOAuthFetchError::Unauthorized) => true,
            CodexOAuthError::Fetch(
                CodexOAuthFetchError::InvalidResponse
                | CodexOAuthFetchError::ServerError(_, _)
                | CodexOAuthFetchError::NetworkError(_)
                | CodexOAuthFetchError::Cancelled,
            ) => false,
            CodexOAuthError::Credentials(
                CodexOAuthCredentialsError::NotFound
                | CodexOAuthCredentialsError::Unreadable
                | CodexOAuthCredentialsError::MissingTokens
                | CodexOAuthCredentialsError::NativeRefreshRequired,
            ) => true,
            CodexOAuthError::Credentials(
                CodexOAuthCredentialsError::DecodeFailed(_)
                | CodexOAuthCredentialsError::ReadOnlySource
                | CodexOAuthCredentialsError::WriteFailed
                | CodexOAuthCredentialsError::Cancelled,
            ) => false,
            CodexOAuthError::Refresh(
                CodexRefreshError::Expired | CodexRefreshError::Revoked | CodexRefreshError::Reused,
            ) => true,
            CodexOAuthError::Refresh(
                CodexRefreshError::NetworkError(_)
                | CodexRefreshError::InvalidResponse(_)
                | CodexRefreshError::Cancelled,
            ) => false,
            CodexOAuthError::NoRateLimitsFound => false,
        }
    }

    pub fn map_credits(
        response: &CodexUsageResponse,
        updated_at: Timestamp,
        include_credits: bool,
    ) -> Option<CreditsSnapshot> {
        let balance = response.credits.as_ref().and_then(|c| c.balance);
        let credit_limit = response
            .resolved_individual_limit()
            .and_then(|l| l.codex_credit_limit_snapshot(updated_at));
        let credits_available = response
            .credits
            .as_ref()
            .map(|c| c.has_credits && !c.unlimited);
        if balance.is_none() && credit_limit.is_none() && credits_available != Some(true) {
            return None;
        }
        Some(CreditsSnapshot::with_details(
            balance.unwrap_or(0.0),
            Vec::new(),
            updated_at,
            credit_limit,
            balance.is_some(),
            if include_credits || balance.is_some() {
                credits_available
            } else {
                None
            },
            false,
        ))
    }

    pub fn replacing_credits(
        mut result: CodexFetchOutcome,
        credits: Option<CreditsSnapshot>,
    ) -> CodexFetchOutcome {
        result.usage = CodexExtraUsageCost::attaching(result.usage, credits.as_ref());
        result.credits = credits;
        result
    }

    pub fn make_result(
        usage_response: &CodexUsageResponse,
        reset_credits: Option<CodexRateLimitResetCreditsSnapshot>,
        credentials: &CodexOAuthCredentials,
        updated_at: Timestamp,
        include_credits: bool,
        allow_empty_usage_for_reset_credit_enrichment: bool,
        codex_reset_credits_attempted: bool,
    ) -> Result<CodexFetchOutcome, CodexOAuthError> {
        let credits = Self::map_credits(usage_response, updated_at, include_credits);
        let reconciled = CodexReconciledState::from_oauth(usage_response, credentials, updated_at);

        let usage = if let Some(reconciled) = reconciled {
            let confidence = if usage_response
                .rate_limit
                .as_ref()
                .is_some_and(|rl| rl.has_window_decode_failure())
                || usage_response.additional_rate_limits_decode_failed
            {
                Confidence::Unknown
            } else {
                Confidence::Exact
            };
            reconciled.to_usage_snapshot(confidence)
        } else {
            let has_available_resets = reset_credits
                .as_ref()
                .is_some_and(|rc| rc.available_inventory(updated_at).count() > 0);
            if credits.is_none()
                && !has_available_resets
                && !allow_empty_usage_for_reset_credit_enrichment
            {
                return Err(CodexOAuthError::NoRateLimitsFound);
            }
            UsageSnapshot::new(
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
                updated_at,
                Some(CodexReconciledState::oauth_identity(
                    usage_response,
                    credentials,
                )),
                Confidence::Unknown,
            )
            .expect("empty details cannot exceed max sections")
        };

        let usage = CodexExtraUsageCost::attaching(usage, credits.as_ref());
        Ok(CodexFetchOutcome {
            usage,
            credits,
            codex_reset_credits: reset_credits,
            source_label: "oauth".to_owned(),
            strategy_id: CODEX_OAUTH_STRATEGY_ID,
            strategy_kind: SourceKind::Oauth,
            codex_reset_credits_attempted,
            codex_monthly_limit_enrichment_failed: false,
        })
    }

    pub async fn applying_workspace_remaining_balance<F, Fut>(
        result: CodexFetchOutcome,
        usage: &CodexUsageResponse,
        credentials: &CodexOAuthCredentials,
        include_credits: bool,
        now: Timestamp,
        fetcher: F,
    ) -> CodexFetchOutcome
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<
                Output = Result<CodexWorkspaceRemainingBalanceResponse, CodexOAuthFetchError>,
            >,
    {
        Self::try_applying_workspace_remaining_balance(
            result.clone(),
            usage,
            credentials,
            include_credits,
            now,
            fetcher,
        )
        .await
        .unwrap_or(result)
    }

    pub async fn try_applying_workspace_remaining_balance<F, Fut>(
        result: CodexFetchOutcome,
        usage: &CodexUsageResponse,
        credentials: &CodexOAuthCredentials,
        include_credits: bool,
        now: Timestamp,
        fetcher: F,
    ) -> Result<CodexFetchOutcome, CodexOAuthFetchError>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<
                Output = Result<CodexWorkspaceRemainingBalanceResponse, CodexOAuthFetchError>,
            >,
    {
        if !include_credits {
            return Ok(result);
        }
        let Some(credits_details) = &usage.credits else {
            return Ok(result);
        };
        if !credits_details.has_credits
            || credits_details.unlimited
            || credits_details.balance.is_some()
        {
            return Ok(result);
        }
        let Some(account_id) = Self::first_non_empty(&[usage.account_id.as_deref()]) else {
            return Ok(result);
        };
        if let Some(credential_acct) = Self::first_non_empty(&[credentials.account_id.as_deref()])
            && credential_acct != account_id
        {
            return Ok(result);
        }

        let resp = match fetcher(account_id.to_owned()).await {
            Ok(resp) => resp,
            Err(CodexOAuthFetchError::Cancelled) => return Err(CodexOAuthFetchError::Cancelled),
            Err(_) => return Ok(result),
        };
        let Some(balance) = resp.balance else {
            return Ok(result);
        };
        let existing = result.credits.as_ref();
        let credits = CreditsSnapshot::with_details(
            balance,
            existing.map(|c| c.events.clone()).unwrap_or_default(),
            now,
            existing.and_then(|c| c.codex_credit_limit.clone()),
            true,
            Some(true),
            true,
        );
        Ok(Self::replacing_credits(result, Some(credits)))
    }

    pub async fn apply_workspace_remaining_balance_for_testing<F, Fut>(
        result: CodexFetchOutcome,
        usage: &CodexUsageResponse,
        credentials: &CodexOAuthCredentials,
        include_credits: bool,
        now: Timestamp,
        fetcher: F,
    ) -> Result<CodexFetchOutcome, CodexOAuthFetchError>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<
                Output = Result<CodexWorkspaceRemainingBalanceResponse, CodexOAuthFetchError>,
            >,
    {
        Self::try_applying_workspace_remaining_balance(
            result,
            usage,
            credentials,
            include_credits,
            now,
            fetcher,
        )
        .await
    }

    pub async fn applying_spend_controls_monthly_limit<F, Fut>(
        result: CodexFetchOutcome,
        usage: &CodexUsageResponse,
        credentials: &CodexOAuthCredentials,
        include_credits: bool,
        fetcher: F,
    ) -> CodexFetchOutcome
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<
                Output = Result<CodexSpendControlsMonthlyUsageResponse, CodexOAuthFetchError>,
            >,
    {
        Self::try_applying_spend_controls_monthly_limit(
            result.clone(),
            usage,
            credentials,
            include_credits,
            fetcher,
        )
        .await
        .unwrap_or_else(|_| result.marking_monthly_limit_enrichment_failed())
    }

    pub async fn try_applying_spend_controls_monthly_limit<F, Fut>(
        result: CodexFetchOutcome,
        usage: &CodexUsageResponse,
        credentials: &CodexOAuthCredentials,
        include_credits: bool,
        fetcher: F,
    ) -> Result<CodexFetchOutcome, CodexOAuthFetchError>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<
                Output = Result<CodexSpendControlsMonthlyUsageResponse, CodexOAuthFetchError>,
            >,
    {
        if !include_credits || !CodexSpendControlsMonthlyUsageGate::should_fetch(usage) {
            return Ok(result);
        }
        let Some(account_id) = Self::first_non_empty(&[
            credentials.account_id.as_deref(),
            usage.account_id.as_deref(),
        ]) else {
            return Ok(result.marking_monthly_limit_enrichment_failed());
        };

        let response = match fetcher(account_id.to_owned()).await {
            Ok(resp) => resp,
            Err(CodexOAuthFetchError::Cancelled) => return Err(CodexOAuthFetchError::Cancelled),
            Err(_) => return Ok(result.marking_monthly_limit_enrichment_failed()),
        };
        if response.monthly_limit_mapping_failed() {
            return Ok(result.marking_monthly_limit_enrichment_failed());
        }
        let updated_at = result
            .credits
            .as_ref()
            .map(|c| c.updated_at)
            .unwrap_or(result.usage.updated_at);
        let Some(limit) = response.codex_credit_limit_snapshot(updated_at) else {
            return Ok(result);
        };
        let credits = match &result.credits {
            Some(existing) => CreditsSnapshot::with_details(
                existing.remaining,
                existing.events.clone(),
                existing.updated_at,
                Some(limit),
                existing.balance_read_succeeded,
                existing.credits_available,
                existing.balance_is_workspace,
            ),
            None => CreditsSnapshot::with_details(
                0.0,
                Vec::new(),
                updated_at,
                Some(limit),
                false,
                None,
                false,
            ),
        };
        Ok(Self::replacing_credits(result, Some(credits)))
    }

    pub async fn apply_spend_controls_monthly_limit_for_testing<F, Fut>(
        result: CodexFetchOutcome,
        usage: &CodexUsageResponse,
        credentials: &CodexOAuthCredentials,
        include_credits: bool,
        fetcher: F,
    ) -> Result<CodexFetchOutcome, CodexOAuthFetchError>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<
                Output = Result<CodexSpendControlsMonthlyUsageResponse, CodexOAuthFetchError>,
            >,
    {
        Self::try_applying_spend_controls_monthly_limit(
            result,
            usage,
            credentials,
            include_credits,
            fetcher,
        )
        .await
    }

    pub async fn fetch_reset_credits_if_requested<F, Fut>(
        ctx: &FetchContext,
        credentials: &CodexOAuthCredentials,
        fetcher: F,
    ) -> Result<Option<CodexRateLimitResetCreditsSnapshot>, CodexOAuthFetchError>
    where
        F: FnOnce(CodexOAuthCredentials) -> Fut,
        Fut: std::future::Future<
                Output = Result<CodexRateLimitResetCreditsSnapshot, CodexOAuthFetchError>,
            >,
    {
        if !Self::should_fetch_reset_credits(ctx) {
            return Ok(None);
        }
        match fetcher(credentials.clone()).await {
            Ok(snap) => Ok(Some(snap)),
            Err(CodexOAuthFetchError::Cancelled) => Err(CodexOAuthFetchError::Cancelled),
            Err(_) => Ok(None),
        }
    }

    pub async fn fetch_reset_credits_for_testing<F, Fut>(
        ctx: &FetchContext,
        credentials: &CodexOAuthCredentials,
        fetcher: F,
    ) -> Result<Option<CodexRateLimitResetCreditsSnapshot>, CodexOAuthFetchError>
    where
        F: FnOnce(CodexOAuthCredentials) -> Fut,
        Fut: std::future::Future<
                Output = Result<CodexRateLimitResetCreditsSnapshot, CodexOAuthFetchError>,
            >,
    {
        Self::fetch_reset_credits_if_requested(ctx, credentials, fetcher).await
    }

    pub async fn replacing_with_cli_monthly_limit_if_available<F, Fut>(
        oauth_result: CodexFetchOutcome,
        source_mode: SourceMode,
        codex_workspace_id: Option<&str>,
        include_credits: bool,
        cli_is_available: bool,
        cli_fetch: F,
    ) -> CodexFetchOutcome
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<CodexFetchOutcome, CodexCLIFetchError>>,
    {
        if source_mode != SourceMode::Auto
            || codex_workspace_id.is_some()
            || !include_credits
            || !Self::should_try_cli_for_monthly_limit(&oauth_result)
            || !cli_is_available
        {
            return oauth_result;
        }
        let Ok(cli_result) = cli_fetch().await else {
            return oauth_result;
        };
        let Some(cli_limit) = cli_result
            .credits
            .as_ref()
            .and_then(|c| c.codex_credit_limit.clone())
        else {
            return oauth_result;
        };
        if !Self::identities_are_compatible(
            oauth_result.usage.identity.as_ref(),
            cli_result.usage.identity.as_ref(),
        ) {
            return oauth_result;
        }
        let Some(oauth_credits) = oauth_result.credits.as_ref() else {
            return oauth_result;
        };
        let updated_credits = CreditsSnapshot::with_details(
            oauth_credits.remaining,
            oauth_credits.events.clone(),
            oauth_credits.updated_at,
            Some(cli_limit),
            oauth_credits.balance_read_succeeded,
            oauth_credits.credits_available,
            oauth_credits.balance_is_workspace,
        );
        Self::replacing_credits(oauth_result, Some(updated_credits))
    }

    pub fn should_try_cli_for_monthly_limit(result: &CodexFetchOutcome) -> bool {
        let Some(credits) = &result.credits else {
            return false;
        };
        let reset_count = result
            .codex_reset_credits
            .as_ref()
            .map(|rc| rc.available_inventory(result.usage.updated_at).count())
            .unwrap_or(0);
        credits.remaining == 0.0 && credits.codex_credit_limit.is_none() && reset_count == 0
    }

    pub fn identities_are_compatible(
        oauth: Option<&ProviderIdentitySnapshot>,
        cli: Option<&ProviderIdentitySnapshot>,
    ) -> bool {
        let Some(oauth_email) = oauth
            .and_then(|i| i.account_email.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
        else {
            return false;
        };
        let Some(cli_email) = cli
            .and_then(|i| i.account_email.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
        else {
            return false;
        };
        oauth_email == cli_email
    }

    fn first_non_empty<'a>(candidates: &[Option<&'a str>]) -> Option<&'a str> {
        for candidate in candidates.iter().flatten() {
            let trimmed = candidate.trim();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
        None
    }

    pub async fn fetch_outcome(
        &self,
        ctx: &FetchContext,
    ) -> Result<CodexFetchOutcome, CodexOAuthError> {
        let call = BrokerCall {
            interaction: ctx.interaction,
            cancel: ctx.cancel.clone(),
            request_id: ctx.request_id.clone(),
        };
        let initial_credentials = self.load_credentials_with_call(true, Some(&call)).await?;
        self.fetch_with_credentials(ctx, initial_credentials).await
    }

    pub async fn fetch_with_credentials(
        &self,
        ctx: &FetchContext,
        initial_credentials: CodexOAuthCredentials,
    ) -> Result<CodexFetchOutcome, CodexOAuthError> {
        let call = BrokerCall {
            interaction: ctx.interaction,
            cancel: ctx.cancel.clone(),
            request_id: ctx.request_id.clone(),
        };
        if call.cancel.is_cancelled() {
            return Err(CodexOAuthError::Fetch(CodexOAuthFetchError::Cancelled));
        }

        let now = (self.config.clock)();
        let mut credentials = Self::prepare_credentials_for_usage(&initial_credentials, now)?;
        if let Some(managed_workspace_id) = self
            .config
            .managed_workspace_account_id
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            credentials = credentials.with_account_id(Some(managed_workspace_id.to_owned()));
        }

        let usage = CodexOAuthUsageFetcher::fetch_usage_with_call(
            &self.config.net,
            credentials.access_token_str(),
            credentials.account_id.as_deref(),
            &self.config.environment,
            Some(&self.config.home_directory),
            Some(&call),
        )
        .await?;

        let reset_credits_attempted = Self::should_fetch_reset_credits(ctx);
        let net_rc = Arc::clone(&self.config.net);
        let env_rc = self.config.environment.clone();
        let home_rc = self.config.home_directory.clone();
        let call_rc = call.clone();
        let reset_credits = Self::fetch_reset_credits_if_requested(ctx, &credentials, |creds| {
            let net = Arc::clone(&net_rc);
            let env = env_rc.clone();
            let home = home_rc.clone();
            let call = call_rc.clone();
            async move {
                CodexOAuthUsageFetcher::fetch_rate_limit_reset_credits_with_call(
                    &net,
                    creds.access_token_str(),
                    creds.account_id.as_deref(),
                    &env,
                    Some(&home),
                    Duration::from_secs(4),
                    now,
                    Some(&call),
                )
                .await
            }
        })
        .await?;

        let oauth_result = Self::make_result(
            &usage,
            reset_credits,
            &credentials,
            now,
            ctx.include_credits,
            ctx.runtime == FetchRuntime::Daemon,
            reset_credits_attempted,
        )?;

        let net = Arc::clone(&self.config.net);
        let env = self.config.environment.clone();
        let home = self.config.home_directory.clone();
        let access_token = credentials.access_token_str().to_owned();
        let call_ws = call.clone();
        let workspace_balance_result = Self::try_applying_workspace_remaining_balance(
            oauth_result,
            &usage,
            &credentials,
            ctx.include_credits,
            now,
            |acct_id| {
                let net = Arc::clone(&net);
                let env = env.clone();
                let home = home.clone();
                let token = access_token.clone();
                let call = call_ws.clone();
                async move {
                    CodexOAuthUsageFetcher::fetch_workspace_remaining_balance_with_call(
                        &net,
                        &token,
                        &acct_id,
                        &env,
                        Some(&home),
                        Duration::from_secs(4),
                        Some(&call),
                    )
                    .await
                }
            },
        )
        .await?;
        if call.cancel.is_cancelled() {
            return Err(CodexOAuthError::Fetch(CodexOAuthFetchError::Cancelled));
        }

        let net2 = Arc::clone(&self.config.net);
        let env2 = self.config.environment.clone();
        let home2 = self.config.home_directory.clone();
        let access_token2 = credentials.access_token_str().to_owned();
        let call_sc = call.clone();
        let spend_controls_result = Self::try_applying_spend_controls_monthly_limit(
            workspace_balance_result,
            &usage,
            &credentials,
            ctx.include_credits,
            |acct_id| {
                let net = Arc::clone(&net2);
                let env = env2.clone();
                let home = home2.clone();
                let token = access_token2.clone();
                let call = call_sc.clone();
                async move {
                    CodexOAuthUsageFetcher::fetch_spend_controls_monthly_usage_with_call(
                        &net,
                        &token,
                        &acct_id,
                        &env,
                        Some(&home),
                        Duration::from_secs(4),
                        Some(&call),
                    )
                    .await
                }
            },
        )
        .await?;
        if call.cancel.is_cancelled() {
            return Err(CodexOAuthError::Fetch(CodexOAuthFetchError::Cancelled));
        }

        let cli_strategy = CodexCLIUsageStrategy::new(self.config.clone());
        let cli_avail = cli_strategy.is_available_sync();
        let include_credits = ctx.include_credits;
        let call_cli = call.clone();
        let final_result = Self::replacing_with_cli_monthly_limit_if_available(
            spend_controls_result,
            ctx.source_mode,
            self.config.managed_workspace_account_id.as_deref(),
            include_credits,
            cli_avail,
            || async move {
                cli_strategy
                    .fetch_outcome_with_call(include_credits, now, Some(call_cli))
                    .await
            },
        )
        .await;
        if call.cancel.is_cancelled() {
            return Err(CodexOAuthError::Fetch(CodexOAuthFetchError::Cancelled));
        }

        Ok(final_result)
    }

    pub fn map_usage_for_testing(
        data: &[u8],
        credentials: &CodexOAuthCredentials,
        now: Timestamp,
    ) -> Result<Option<UsageSnapshot>, CodexOAuthFetchError> {
        let usage = CodexOAuthUsageFetcher::decode_usage_response(data)?;
        Ok(CodexReconciledState::from_oauth(&usage, credentials, now)
            .map(|s| s.to_usage_snapshot(Confidence::Exact)))
    }

    pub fn map_result_for_testing(
        data: &[u8],
        credentials: &CodexOAuthCredentials,
        reset_credits: Option<CodexRateLimitResetCreditsSnapshot>,
        allow_empty_usage_for_reset_credit_enrichment: bool,
        include_credits: bool,
        now: Timestamp,
    ) -> Result<CodexFetchOutcome, CodexOAuthError> {
        let usage_response = CodexOAuthUsageFetcher::decode_usage_response(data)?;
        Self::make_result(
            &usage_response,
            reset_credits,
            credentials,
            now,
            include_credits,
            allow_empty_usage_for_reset_credit_enrichment,
            false,
        )
    }
}

impl FetchStrategy for CodexOAuthFetchStrategy {
    fn id(&self) -> &'static str {
        CODEX_OAUTH_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Oauth
    }

    fn is_available<'a>(&'a self, ctx: &'a FetchContext) -> BoxFuture<'a, bool> {
        let call = BrokerCall {
            interaction: ctx.interaction,
            cancel: ctx.cancel.clone(),
            request_id: ctx.request_id.clone(),
        };
        Box::pin(async move {
            self.load_credentials_with_call(false, Some(&call))
                .await
                .is_ok()
        })
    }

    fn fetch<'a>(
        &'a self,
        ctx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            self.fetch_outcome(ctx)
                .await
                .map(CodexFetchOutcome::into_fetch_result)
                .map_err(CodexOAuthError::into_fetch_error)
        })
    }

    fn should_fallback(&self, err: &FetchError, ctx: &FetchContext) -> bool {
        let FetchError::Classified(classified) = err else {
            return false;
        };
        let is_explicit_native_refresh =
            classified.message == CodexOAuthCredentialsError::NativeRefreshRequired.to_string();
        if ctx.source_mode != SourceMode::Auto
            && !(ctx.source_mode == SourceMode::Oauth && is_explicit_native_refresh)
        {
            return false;
        }
        matches!(
            classified.kind,
            ProviderErrorKind::AuthenticationExpired
                | ProviderErrorKind::MissingCredential
                | ProviderErrorKind::PermissionDenied
        ) && classified.message != CodexOAuthCredentialsError::ReadOnlySource.to_string()
    }
}

#[derive(Clone)]
pub struct CodexOAuthNativeRefreshCLIStrategy {
    pub config: CodexStrategyConfig,
    pub binary_resolver: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl CodexOAuthNativeRefreshCLIStrategy {
    pub fn new(config: CodexStrategyConfig) -> Self {
        Self {
            config,
            binary_resolver: None,
        }
    }

    pub fn with_binary_resolver(
        mut self,
        resolver: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        self.binary_resolver = Some(Arc::new(resolver));
        self
    }

    pub fn is_available_sync(&self, source_mode: SourceMode) -> bool {
        if self.config.managed_workspace_account_id.is_some() || source_mode != SourceMode::Oauth {
            return false;
        }
        let has_binary = match &self.binary_resolver {
            Some(resolver) => resolver(),
            None => {
                self.config.cli_fetcher.resolved_executable.is_some()
                    || CodexCLIUsageFetcher::resolve_binary(
                        &self.config.environment,
                        Some(&self.config.home_directory),
                    )
                    .is_some()
            }
        };
        if !has_binary {
            return false;
        }
        let Ok(credentials) = CodexOAuthCredentialsStore::load_for_usage(
            &self.config.environment,
            self.config.allow_external_oauth_sources,
            Some(&self.config.home_directory),
        ) else {
            return false;
        };
        let now = (self.config.clock)();
        credentials.source == CodexOAuthCredentialSource::CodexHome
            && credentials.needs_refresh_at(now)
    }
}

impl FetchStrategy for CodexOAuthNativeRefreshCLIStrategy {
    fn id(&self) -> &'static str {
        CODEX_OAUTH_NATIVE_REFRESH_CLI_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Cli
    }

    fn is_available<'a>(&'a self, ctx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move { self.is_available_sync(ctx.source_mode) })
    }

    fn fetch<'a>(
        &'a self,
        ctx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        let cli = CodexCLIUsageStrategy::new(self.config.clone());
        Box::pin(async move { cli.fetch(ctx).await })
    }

    fn should_fallback(&self, _err: &FetchError, _ctx: &FetchContext) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct CodexCLIUsageStrategy {
    pub config: CodexStrategyConfig,
}

impl CodexCLIUsageStrategy {
    pub fn new(config: CodexStrategyConfig) -> Self {
        Self { config }
    }

    pub fn is_available_sync(&self) -> bool {
        self.config.cli_fetcher.resolved_executable.is_some()
            || CodexCLIUsageFetcher::resolve_binary(
                &self.config.environment,
                Some(&self.config.home_directory),
            )
            .is_some()
    }

    pub async fn fetch_outcome(
        &self,
        include_credits: bool,
        now: Timestamp,
    ) -> Result<CodexFetchOutcome, CodexCLIFetchError> {
        self.fetch_outcome_with_call(include_credits, now, None)
            .await
    }

    pub async fn fetch_outcome_with_call(
        &self,
        include_credits: bool,
        now: Timestamp,
        call: Option<BrokerCall>,
    ) -> Result<CodexFetchOutcome, CodexCLIFetchError> {
        let snapshot = self
            .config
            .cli_fetcher
            .load_latest_cli_account_snapshot_with_call(now, call)
            .await?;
        let Some(usage) = snapshot.usage else {
            if !include_credits {
                return Err(CodexCLIFetchError::Usage(UsageError::NoRateLimitsFound));
            }
            let Some(credits) = snapshot.credits else {
                return Err(CodexCLIFetchError::Usage(UsageError::NoRateLimitsFound));
            };
            let empty_usage = UsageSnapshot::new(
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
                credits.updated_at,
                snapshot.identity,
                Confidence::Unknown,
            )
            .expect("empty details cannot exceed max sections");
            let attached = CodexExtraUsageCost::attaching(empty_usage, Some(&credits));
            return Ok(CodexFetchOutcome {
                usage: attached,
                credits: Some(credits),
                codex_reset_credits: None,
                source_label: "codex-cli".to_owned(),
                strategy_id: CODEX_CLI_STRATEGY_ID,
                strategy_kind: SourceKind::Cli,
                codex_reset_credits_attempted: false,
                codex_monthly_limit_enrichment_failed: false,
            });
        };
        let credits = if include_credits {
            snapshot.credits
        } else {
            None
        };
        let attached = CodexExtraUsageCost::attaching(usage, credits.as_ref());
        Ok(CodexFetchOutcome {
            usage: attached,
            credits,
            codex_reset_credits: None,
            source_label: "codex-cli".to_owned(),
            strategy_id: CODEX_CLI_STRATEGY_ID,
            strategy_kind: SourceKind::Cli,
            codex_reset_credits_attempted: false,
            codex_monthly_limit_enrichment_failed: false,
        })
    }
}

impl FetchStrategy for CodexCLIUsageStrategy {
    fn id(&self) -> &'static str {
        CODEX_CLI_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Cli
    }

    fn is_available<'a>(&'a self, _ctx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move { self.is_available_sync() })
    }

    fn fetch<'a>(
        &'a self,
        ctx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        let now = (self.config.clock)();
        let include_credits = ctx.include_credits;
        let call = BrokerCall {
            interaction: ctx.interaction,
            cancel: ctx.cancel.clone(),
            request_id: ctx.request_id.clone(),
        };
        Box::pin(async move {
            self.fetch_outcome_with_call(include_credits, now, Some(call))
                .await
                .map(CodexFetchOutcome::into_fetch_result)
                .map_err(CodexCLIFetchError::into_fetch_error)
        })
    }

    fn should_fallback(&self, _err: &FetchError, _ctx: &FetchContext) -> bool {
        false
    }
}

#[derive(Clone, Default)]
pub struct CodexPATFetchStrategy {
    pub config: Option<CodexStrategyConfig>,
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

impl CodexPATFetchStrategy {
    pub fn new() -> Self {
        Self { config: None }
    }

    pub fn with_config(config: CodexStrategyConfig) -> Self {
        Self {
            config: Some(config),
        }
    }

    pub fn resolved_cli_version(&self) -> Option<String> {
        self.config
            .as_ref()
            .and_then(Self::resolved_cli_version_for_testing)
    }

    pub async fn resolve_cli_version_with_call(&self, call: Option<&BrokerCall>) -> Option<String> {
        if let Some(version) = self.resolved_cli_version() {
            return Some(version);
        }
        if let Some(config) = &self.config {
            return config.cli_fetcher.detect_cli_version(call).await;
        }
        None
    }

    pub fn resolved_cli_version_for_testing(config: &CodexStrategyConfig) -> Option<String> {
        config
            .resolved_cli_version
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }

    /// PAT lives in the Codex CLI auth file. Managed-account `CODEX_HOME` isolation is for OAuth
    /// workspaces and must not hide that token, including the fail-closed dummy home used when a
    /// persisted managed account no longer exists. Profile homes keep a local PAT when present and
    /// otherwise fall through to ambient `~/.codex`.
    pub fn credential_environment(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        Self::credential_environment_with_reader_and_call(env, None, None)
    }

    pub fn credential_environment_for_testing(
        env: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        Self::credential_environment(env)
    }

    pub fn credential_environment_with_reader_and_call(
        env: &BTreeMap<String, String>,
        reader: Option<&CredentialReadFn>,
        call: Option<&BrokerCall>,
    ) -> BTreeMap<String, String> {
        let Some(home) = env.get("CODEX_HOME") else {
            return env.clone();
        };
        if Self::is_managed_or_fail_closed_codex_home(home) {
            return Self::ambient_credential_environment(env);
        }
        if !home.trim().is_empty()
            && CodexOAuthCredentialsStore::load_pat_with_reader_and_call(env, None, reader, call)
                .is_ok()
        {
            return env.clone();
        }
        Self::ambient_credential_environment(env)
    }

    fn ambient_credential_environment(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut ambient = env.clone();
        // `loadPAT` only honors `CODEX_HOME`. Stripping a managed home without pointing at
        // `$HOME/.codex` falls through to the process user's real home, which hides a PAT
        // that tests (and some launchd/sudo environments) keep under a different HOME.
        if let Some(home) = env.get("HOME").map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let codex_home = normalize_lexical_path(&PathBuf::from(home).join(".codex"));
            ambient.insert(
                "CODEX_HOME".to_owned(),
                codex_home.to_string_lossy().into_owned(),
            );
        } else {
            ambient.remove("CODEX_HOME");
        }
        ambient
    }

    pub fn is_managed_or_fail_closed_codex_home(path: &str) -> bool {
        let normalized = normalize_lexical_path(Path::new(path));
        if normalized.file_name().and_then(|s| s.to_str()) == Some("managed-store-unreadable") {
            return true;
        }
        normalized
            .components()
            .any(|c| c.as_os_str().to_str() == Some("managed-codex-homes"))
    }

    fn base_environment(&self) -> BTreeMap<String, String> {
        self.config
            .as_ref()
            .map(|c| c.environment.clone())
            .unwrap_or_default()
    }

    pub fn make_result(
        usage_response: &CodexUsageResponse,
        whoami: Option<&CodexPATWhoami>,
        updated_at: Timestamp,
    ) -> Result<CodexFetchOutcome, CodexOAuthError> {
        let credits = CodexOAuthFetchStrategy::map_credits(usage_response, updated_at, true);
        let reconciled = CodexReconciledState::from_pat(usage_response, whoami, updated_at);

        let usage = if let Some(reconciled) = reconciled {
            let confidence = if usage_response
                .rate_limit
                .as_ref()
                .is_some_and(|rl| rl.has_window_decode_failure())
                || usage_response.additional_rate_limits_decode_failed
            {
                Confidence::Unknown
            } else {
                Confidence::Exact
            };
            reconciled.to_usage_snapshot(confidence)
        } else {
            if credits.is_none() {
                return Err(CodexOAuthError::NoRateLimitsFound);
            }
            UsageSnapshot::new(
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
                updated_at,
                Some(CodexReconciledState::pat_identity(usage_response, whoami)),
                Confidence::Unknown,
            )
            .expect("empty details cannot exceed max sections")
        };

        let usage = CodexExtraUsageCost::attaching(usage, credits.as_ref());
        Ok(CodexFetchOutcome {
            usage,
            credits,
            codex_reset_credits: None,
            source_label: CodexUsageDataSource::Pat.source_label().to_owned(),
            strategy_id: CODEX_PAT_STRATEGY_ID,
            strategy_kind: SourceKind::Api,
            codex_reset_credits_attempted: true,
            codex_monthly_limit_enrichment_failed: false,
        })
    }

    pub fn map_result_for_testing(
        data: &[u8],
        whoami: Option<&CodexPATWhoami>,
        updated_at: Timestamp,
    ) -> Result<CodexFetchOutcome, CodexOAuthError> {
        let usage_response = CodexOAuthUsageFetcher::decode_usage_response(data)?;
        Self::make_result(&usage_response, whoami, updated_at)
    }

    pub async fn fetch_outcome(
        &self,
        ctx: &FetchContext,
    ) -> Result<CodexFetchOutcome, CodexOAuthError> {
        let call = BrokerCall {
            interaction: ctx.interaction,
            cancel: ctx.cancel.clone(),
            request_id: ctx.request_id.clone(),
        };
        let base_env = self.base_environment();
        let credential_env =
            Self::credential_environment_with_reader_and_call(&base_env, None, Some(&call));
        let home_dir = self.config.as_ref().map(|c| c.home_directory.as_path());
        let credentials = CodexOAuthCredentialsStore::load_pat_with_reader_and_call(
            &credential_env,
            home_dir,
            None,
            Some(&call),
        )?;
        let cli_version = self.resolve_cli_version_with_call(Some(&call)).await;
        let fallback_net;
        let net: &NetBroker = if let Some(config) = &self.config {
            &config.net
        } else {
            fallback_net = NetBroker::new(NetConfig::default()).map_err(|_| {
                CodexOAuthError::Fetch(CodexOAuthFetchError::NetworkError("transport".to_owned()))
            })?;
            &fallback_net
        };
        let fetched = CodexPATUsageFetcher::fetch_usage_with_call(
            net,
            &credentials,
            cli_version.as_deref(),
            &credential_env,
            home_dir,
            Some(&call),
        )
        .await?;
        let now = self
            .config
            .as_ref()
            .map_or_else(Timestamp::now, |c| (c.clock)());
        Self::make_result(&fetched.usage, fetched.whoami.as_ref(), now)
    }

    pub fn should_fallback_on_error(error: &CodexOAuthError, source_mode: SourceMode) -> bool {
        if source_mode != SourceMode::Auto {
            return false;
        }
        match error {
            CodexOAuthError::Fetch(CodexOAuthFetchError::Unauthorized) => true,
            CodexOAuthError::Fetch(
                CodexOAuthFetchError::InvalidResponse
                | CodexOAuthFetchError::ServerError(_, _)
                | CodexOAuthFetchError::NetworkError(_)
                | CodexOAuthFetchError::Cancelled,
            ) => false,
            CodexOAuthError::Credentials(
                CodexOAuthCredentialsError::NotFound
                | CodexOAuthCredentialsError::Unreadable
                | CodexOAuthCredentialsError::MissingTokens,
            ) => true,
            CodexOAuthError::Credentials(
                CodexOAuthCredentialsError::DecodeFailed(_)
                | CodexOAuthCredentialsError::ReadOnlySource
                | CodexOAuthCredentialsError::NativeRefreshRequired
                | CodexOAuthCredentialsError::WriteFailed
                | CodexOAuthCredentialsError::Cancelled,
            ) => false,
            CodexOAuthError::Refresh(_) | CodexOAuthError::NoRateLimitsFound => false,
        }
    }
}

impl FetchStrategy for CodexPATFetchStrategy {
    fn id(&self) -> &'static str {
        CODEX_PAT_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Api
    }

    fn is_available<'a>(&'a self, ctx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let call = BrokerCall {
                interaction: ctx.interaction,
                cancel: ctx.cancel.clone(),
                request_id: ctx.request_id.clone(),
            };
            if call.cancel.is_cancelled() {
                return false;
            }
            let base_env = self.base_environment();
            let credential_env =
                Self::credential_environment_with_reader_and_call(&base_env, None, Some(&call));
            let home_dir = self.config.as_ref().map(|c| c.home_directory.as_path());
            CodexOAuthCredentialsStore::load_pat_with_reader_and_call(
                &credential_env,
                home_dir,
                None,
                Some(&call),
            )
            .is_ok()
        })
    }

    fn fetch<'a>(
        &'a self,
        ctx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            self.fetch_outcome(ctx)
                .await
                .map(CodexFetchOutcome::into_fetch_result)
                .map_err(CodexOAuthError::into_fetch_error)
        })
    }

    fn should_fallback(&self, err: &FetchError, ctx: &FetchContext) -> bool {
        if ctx.source_mode != SourceMode::Auto {
            return false;
        }
        let FetchError::Classified(classified) = err else {
            return false;
        };
        matches!(
            classified.kind,
            ProviderErrorKind::AuthenticationExpired
                | ProviderErrorKind::MissingCredential
                | ProviderErrorKind::PermissionDenied
        ) && (classified.message == CodexOAuthFetchError::Unauthorized.to_string()
            || classified.message == CodexOAuthCredentialsError::NotFound.to_string()
            || classified.message == CodexOAuthCredentialsError::Unreadable.to_string()
            || classified.message == CodexOAuthCredentialsError::MissingTokens.to_string())
    }
}

#[derive(Clone, Default)]
pub struct CodexWebDashboardStrategy;

impl CodexWebDashboardStrategy {
    pub fn new() -> Self {
        Self
    }
}

impl FetchStrategy for CodexWebDashboardStrategy {
    fn id(&self) -> &'static str {
        CODEX_WEB_DASHBOARD_STRATEGY_ID
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Web
    }

    fn is_available<'a>(&'a self, _ctx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move { false })
    }

    fn fetch<'a>(
        &'a self,
        _ctx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            Err(FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::ProviderUnavailable,
                "Codex web dashboard source is not yet implemented.",
            )))
        })
    }

    fn should_fallback(&self, _err: &FetchError, _ctx: &FetchContext) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct CodexProviderPlan {
    pub config: CodexStrategyConfig,
}

impl CodexProviderPlan {
    pub const SOURCE_MODES: &'static [SourceMode] = &[
        SourceMode::Auto,
        SourceMode::Web,
        SourceMode::Cli,
        SourceMode::Oauth,
        SourceMode::Api,
    ];

    pub fn new(config: CodexStrategyConfig) -> Self {
        Self { config }
    }

    pub fn provider_id(&self) -> ProviderId {
        ProviderId::new("codex").expect("valid provider id")
    }

    pub fn source_modes(&self) -> &'static [SourceMode] {
        Self::SOURCE_MODES
    }

    pub fn resolve_strategy_ids(&self, source_mode: SourceMode) -> Vec<&'static str> {
        match source_mode {
            SourceMode::Oauth => vec![
                CODEX_OAUTH_STRATEGY_ID,
                CODEX_OAUTH_NATIVE_REFRESH_CLI_STRATEGY_ID,
            ],
            SourceMode::Web => vec![CODEX_WEB_DASHBOARD_STRATEGY_ID],
            SourceMode::Cli => vec![CODEX_CLI_STRATEGY_ID],
            SourceMode::Api => vec![CODEX_PAT_STRATEGY_ID],
            SourceMode::Auto => {
                if self.config.managed_workspace_account_id.is_none() {
                    vec![
                        CODEX_PAT_STRATEGY_ID,
                        CODEX_OAUTH_STRATEGY_ID,
                        CODEX_CLI_STRATEGY_ID,
                    ]
                } else {
                    vec![CODEX_PAT_STRATEGY_ID, CODEX_OAUTH_STRATEGY_ID]
                }
            }
        }
    }
}

impl ProviderPlan for CodexProviderPlan {
    fn resolve_strategies<'a>(
        &'a self,
        ctx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        Box::pin(async move {
            let pat: Box<dyn FetchStrategy> =
                Box::new(CodexPATFetchStrategy::with_config(self.config.clone()));
            let oauth: Box<dyn FetchStrategy> =
                Box::new(CodexOAuthFetchStrategy::new(self.config.clone()));
            let cli: Box<dyn FetchStrategy> =
                Box::new(CodexCLIUsageStrategy::new(self.config.clone()));
            let web: Box<dyn FetchStrategy> = Box::new(CodexWebDashboardStrategy::new());

            match ctx.source_mode {
                SourceMode::Oauth => {
                    let native_refresh: Box<dyn FetchStrategy> =
                        Box::new(CodexOAuthNativeRefreshCLIStrategy::new(self.config.clone()));
                    vec![oauth, native_refresh]
                }
                SourceMode::Web => vec![web],
                SourceMode::Cli => vec![cli],
                SourceMode::Api => vec![pat],
                SourceMode::Auto => {
                    if self.config.managed_workspace_account_id.is_none() {
                        vec![pat, oauth, cli]
                    } else {
                        vec![pat, oauth]
                    }
                }
            }
        })
    }
}
