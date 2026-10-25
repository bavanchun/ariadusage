// Ported from CodexBar Sources/CodexBarCore/UsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexStatusProbe.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::model::identity::ProviderIdentitySnapshot;
use ariadusage_core::model::snapshot::UsageSnapshot;
use ariadusage_core::model::window::RateWindow;
use ariadusage_core::pipeline::{ClassifiedError, FetchError, FetchInteraction, TransportClass};
use ariadusage_protocol::usage::ProviderErrorKind;
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::brokers::call::BrokerCall;
use crate::brokers::exec_resolver::{RealResolverFs, ResolverInputs, resolve_codex_for_rpc};
use crate::brokers::process::{
    AbsolutePath, Command, ProcessEnv, ProcessRegistry, RpcError, RpcSession,
};

use super::cli_backend_config::CodexCLIBackendRateLimitError;
use super::cli_launch_gate::CodexCLILaunchGate;
use super::credentials::CodexOAuthCredentialsStore;
use super::credits::{CodexCreditLimitSnapshot, CreditsSnapshot};
use super::identity::CodexAuthBackedAccount;
use super::normalizer::WindowSnapshot;
use super::reconciled::CodexReconciledState;
use super::status_probe::{CodexCLISession, CodexStatusProbe};
use super::usage_fetcher::{CodexSpendControlNumber, CreditDetails, RateLimitDetails};

pub const DEFAULT_CODEX_APP_SERVER_ARGS: &[&str] =
    &["-s", "read-only", "-a", "never", "app-server"];

#[derive(Debug, Clone, PartialEq)]
pub struct CodexStatusSnapshot {
    pub credits: Option<f64>,
    pub five_hour_percent_left: Option<i64>,
    pub weekly_percent_left: Option<i64>,
    pub five_hour_reset_description: Option<String>,
    pub weekly_reset_description: Option<String>,
    pub five_hour_resets_at: Option<Timestamp>,
    pub weekly_resets_at: Option<Timestamp>,
    pub codex_credit_limit: Option<CodexCreditLimitSnapshot>,
    pub raw_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodexStatusProbeError {
    #[error(
        "Codex CLI missing. Install via `npm i -g @openai/codex` (or bun install) and restart."
    )]
    CodexNotInstalled,
    #[error("{0}")]
    LaunchBlocked(String),
    #[error("Could not parse Codex status; will retry shortly.")]
    ParseFailed(String),
    #[error("Codex status probe timed out.")]
    TimedOut,
    #[error("Codex CLI update needed: {0}")]
    UpdateRequired(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageError {
    #[error("No Codex sessions found yet. Run at least one Codex prompt first.")]
    NoSessions,
    #[error("Found sessions, but no rate limit events yet.")]
    NoRateLimitsFound,
    #[error("Could not parse Codex session log.")]
    DecodeFailed,
}

impl UsageError {
    pub fn is_no_rate_limits_found_description(text: Option<&str>) -> bool {
        text.map(str::trim) == Some("Found sessions, but no rate limit events yet.")
    }
}

#[derive(Clone, PartialEq, Eq, Default)]
pub struct AccountInfo {
    pub email: Option<String>,
    pub plan: Option<String>,
}

impl std::fmt::Debug for AccountInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountInfo")
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .field("plan", &self.plan)
            .finish()
    }
}

impl AccountInfo {
    pub fn new(email: Option<String>, plan: Option<String>) -> Self {
        Self { email, plan }
    }

    pub fn has_identity(&self) -> bool {
        self.email.as_deref().is_some_and(|s| !s.trim().is_empty())
            || self.plan.as_deref().is_some_and(|s| !s.trim().is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageLimitsAvailability {
    Available,
    Unavailable,
}

impl UsageLimitsAvailability {
    pub fn is_unavailable(self) -> bool {
        self == Self::Unavailable
    }

    pub fn resolve(
        provider: ProviderId,
        snapshot: Option<&UsageSnapshot>,
        account: Option<&AccountInfo>,
        last_error_description: Option<&str>,
    ) -> Self {
        if provider.as_str() == "claude" {
            if let Some(snapshot) = snapshot {
                let has_measured_window = snapshot
                    .primary
                    .as_ref()
                    .is_some_and(|w| !w.is_synthetic_placeholder)
                    || snapshot
                        .secondary
                        .as_ref()
                        .is_some_and(|w| !w.is_synthetic_placeholder)
                    || snapshot
                        .tertiary
                        .as_ref()
                        .is_some_and(|w| !w.is_synthetic_placeholder);
                return if snapshot
                    .primary
                    .as_ref()
                    .is_some_and(|w| w.is_synthetic_placeholder)
                    && !has_measured_window
                {
                    Self::Unavailable
                } else {
                    Self::Available
                };
            }
            return Self::Available;
        }

        if provider.as_str() != "codex" && provider.as_str() != "antigravity" {
            return Self::Available;
        }

        if let Some(snapshot) = snapshot {
            if snapshot.identity_for(&provider).is_none() {
                return Self::Available;
            }
            let has_rate_limit_windows = snapshot.primary.is_some()
                || snapshot.secondary.is_some()
                || snapshot.tertiary.is_some();
            return if has_rate_limit_windows {
                Self::Available
            } else {
                Self::Unavailable
            };
        }

        if provider.as_str() == "codex"
            && UsageError::is_no_rate_limits_found_description(last_error_description)
            && account.is_some_and(AccountInfo::has_identity)
        {
            return Self::Unavailable;
        }
        Self::Available
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexCLIAccountSnapshot {
    pub usage: Option<UsageSnapshot>,
    pub credits: Option<CreditsSnapshot>,
    pub identity: Option<ProviderIdentitySnapshot>,
}

#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub enum RPCWireError {
    #[error("Codex not running. Try running a Codex command first. ({0})")]
    StartFailed(String),
    #[error("Codex connection failed: {}", Self::redact_error_body(.0.as_str()))]
    RequestFailed(zeroize::Zeroizing<String>),
    #[error("Codex returned invalid data: {0}")]
    Malformed(String),
    #[error("Codex RPC timed out waiting for `{method}` reply.")]
    Timeout { method: String },
    #[error("Codex CLI request was cancelled.")]
    Cancelled,
}

impl RPCWireError {
    pub fn redact_error_body(text: &str) -> String {
        if let Some(json_slice) = CodexCLIUsageFetcher::extract_json_object_after("body=", text)
            && let Some(idx) = text.find(json_slice)
        {
            let mut out = String::with_capacity(text.len());
            out.push_str(&text[..idx]);
            out.push_str("[redacted]");
            out.push_str(&text[idx + json_slice.len()..]);
            return out;
        }
        text.to_owned()
    }
}

impl std::fmt::Debug for RPCWireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StartFailed(msg) => f.debug_tuple("StartFailed").field(msg).finish(),
            Self::RequestFailed(msg) => f
                .debug_tuple("RequestFailed")
                .field(&Self::redact_error_body(msg.as_str()))
                .finish(),
            Self::Malformed(msg) => f.debug_tuple("Malformed").field(msg).finish(),
            Self::Timeout { method } => f.debug_struct("Timeout").field("method", method).finish(),
            Self::Cancelled => f.write_str("Cancelled"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodexCLIFetchError {
    #[error(transparent)]
    StatusProbe(#[from] CodexStatusProbeError),
    #[error(transparent)]
    Wire(#[from] RPCWireError),
    #[error(transparent)]
    BackendRateLimit(#[from] CodexCLIBackendRateLimitError),
    #[error(transparent)]
    Usage(#[from] UsageError),
    #[error("Codex CLI fetch was cancelled.")]
    Cancelled,
}

impl CodexCLIFetchError {
    pub fn into_fetch_error(self) -> FetchError {
        match &self {
            Self::Cancelled | Self::Wire(RPCWireError::Cancelled) => FetchError::Cancelled,
            Self::StatusProbe(CodexStatusProbeError::CodexNotInstalled) => FetchError::Classified(
                ClassifiedError::new(ProviderErrorKind::ProviderUnavailable, self.to_string()),
            ),
            Self::StatusProbe(CodexStatusProbeError::LaunchBlocked(_)) => FetchError::Classified(
                ClassifiedError::new(ProviderErrorKind::ProviderUnavailable, self.to_string()),
            ),
            Self::StatusProbe(CodexStatusProbeError::TimedOut)
            | Self::Wire(RPCWireError::Timeout { .. }) => FetchError::Classified(
                ClassifiedError::new(ProviderErrorKind::NetworkFailure, self.to_string())
                    .with_transport(TransportClass::Timeout),
            ),
            Self::StatusProbe(CodexStatusProbeError::ParseFailed(_))
            | Self::Wire(RPCWireError::Malformed(_))
            | Self::Usage(
                UsageError::DecodeFailed | UsageError::NoRateLimitsFound | UsageError::NoSessions,
            ) => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::ParseFailure,
                self.to_string(),
            )),
            Self::StatusProbe(CodexStatusProbeError::UpdateRequired(_))
            | Self::Wire(RPCWireError::StartFailed(_) | RPCWireError::RequestFailed(_)) => {
                FetchError::Classified(ClassifiedError::new(
                    ProviderErrorKind::ProviderUnavailable,
                    self.to_string(),
                ))
            }
            Self::BackendRateLimit(_) => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::AuthenticationExpired,
                self.to_string(),
            )),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum RPCAccountDetails {
    ApiKey,
    ChatGPT { email: String, plan_type: String },
}

impl std::fmt::Debug for RPCAccountDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey => f.write_str("ApiKey"),
            Self::ChatGPT { plan_type, .. } => f
                .debug_struct("ChatGPT")
                .field("email", &"[redacted]")
                .field("plan_type", plan_type)
                .finish(),
        }
    }
}

impl<'de> Deserialize<'de> for RPCAccountDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawAccountDetails {
            #[serde(rename = "type")]
            account_type: String,
            #[serde(default)]
            email: Option<String>,
            #[serde(default, rename = "planType")]
            plan_type: Option<String>,
        }
        let raw = RawAccountDetails::deserialize(deserializer)?;
        match raw.account_type.to_lowercase().as_str() {
            "apikey" => Ok(Self::ApiKey),
            "chatgpt" => Ok(Self::ChatGPT {
                email: raw.email.unwrap_or_else(|| "unknown".to_owned()),
                plan_type: raw.plan_type.unwrap_or_else(|| "unknown".to_owned()),
            }),
            other => Err(serde::de::Error::custom(format!(
                "Unknown account type {other}"
            ))),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct RPCAccountResponse {
    #[serde(default)]
    pub account: Option<RPCAccountDetails>,
    #[serde(default, rename = "requiresOpenaiAuth")]
    pub requires_openai_auth: Option<bool>,
}

impl std::fmt::Debug for RPCAccountResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RPCAccountResponse")
            .field("account", &self.account)
            .field("requires_openai_auth", &self.requires_openai_auth)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RPCRateLimitWindow {
    pub used_percent: f64,
    #[serde(default)]
    pub window_duration_mins: Option<i64>,
    #[serde(default)]
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RPCCreditsSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(default)]
    pub balance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RPCSpendControlLimitSnapshot {
    pub limit: Option<f64>,
    pub used: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub resets_at: Option<i64>,
}

impl<'de> Deserialize<'de> for RPCSpendControlLimitSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        let limit = CodexSpendControlNumber::double(&map, "limit");
        let used = CodexSpendControlNumber::double(&map, "used");
        let remaining_percent = CodexSpendControlNumber::double(&map, "remainingPercent")
            .or_else(|| CodexSpendControlNumber::double(&map, "remaining_percent"));
        let resets_at = CodexSpendControlNumber::integer(&map, "resetsAt")
            .or_else(|| CodexSpendControlNumber::integer(&map, "resets_at"));
        Ok(Self {
            limit,
            used,
            remaining_percent,
            resets_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RPCRateLimitSnapshot {
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub primary: Option<RPCRateLimitWindow>,
    pub secondary: Option<RPCRateLimitWindow>,
    pub credits: Option<RPCCreditsSnapshot>,
    pub individual_limit: Option<RPCSpendControlLimitSnapshot>,
    pub plan_type: Option<String>,
    pub rate_limit_reached_type: Option<String>,
}

impl<'de> Deserialize<'de> for RPCRateLimitSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        let limit_id = decode_opt(&map, "limitId").or_else(|| decode_opt(&map, "limit_id"));
        let limit_name = decode_opt(&map, "limitName").or_else(|| decode_opt(&map, "limit_name"));
        let primary = decode_opt(&map, "primary");
        let secondary = decode_opt(&map, "secondary");
        let credits = decode_opt(&map, "credits");
        let individual_limit =
            decode_opt(&map, "individualLimit").or_else(|| decode_opt(&map, "individual_limit"));
        let plan_type = decode_opt(&map, "planType").or_else(|| decode_opt(&map, "plan_type"));
        let rate_limit_reached_type = decode_opt(&map, "rateLimitReachedType")
            .or_else(|| decode_opt(&map, "rate_limit_reached_type"));
        Ok(Self {
            limit_id,
            limit_name,
            primary,
            secondary,
            credits,
            individual_limit,
            plan_type,
            rate_limit_reached_type,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RPCRateLimitsResponse {
    pub rate_limits: RPCRateLimitSnapshot,
    pub rate_limits_by_limit_id: Option<BTreeMap<String, RPCRateLimitSnapshot>>,
}

impl<'de> Deserialize<'de> for RPCRateLimitsResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        let rl_val = map
            .get("rateLimits")
            .ok_or_else(|| serde::de::Error::missing_field("rateLimits"))?;
        let rate_limits = serde_json::from_value::<RPCRateLimitSnapshot>(rl_val.clone())
            .map_err(serde::de::Error::custom)?;
        let rate_limits_by_limit_id = decode_opt(&map, "rateLimitsByLimitId")
            .or_else(|| decode_opt(&map, "rate_limits_by_limit_id"));
        Ok(Self {
            rate_limits,
            rate_limits_by_limit_id,
        })
    }
}

fn decode_opt<T: for<'de> Deserialize<'de>>(map: &Map<String, Value>, key: &str) -> Option<T> {
    let val = map.get(key)?;
    if val.is_null() {
        return None;
    }
    serde_json::from_value(val.clone()).ok()
}

#[derive(Deserialize)]
struct RPCRateLimitsErrorBody {
    #[serde(default)]
    email: Option<String>,
    #[serde(default, rename = "plan_type")]
    plan_type: Option<String>,
    #[serde(default, rename = "rate_limit")]
    rate_limit: Option<RateLimitDetails>,
    #[serde(default)]
    credits: Option<CreditDetails>,
}

#[derive(Clone)]
pub struct CodexCLIUsageFetcher {
    pub environment: BTreeMap<String, String>,
    pub home_dir: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub working_directory: PathBuf,
    pub initialize_timeout: Duration,
    pub request_timeout: Duration,
    pub codex_arguments: Vec<String>,
    pub resolved_executable: Option<AbsolutePath>,
    pub process_registry: Arc<ProcessRegistry>,
    pub core_launch_gate: Arc<LaunchGate>,
    pub cli_session: CodexCLISession,
    pub cli_launch_gate: Arc<CodexCLILaunchGate>,
    pub interaction: FetchInteraction,
    pub cancel: CancellationToken,
}

impl std::fmt::Debug for CodexCLIUsageFetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexCLIUsageFetcher")
            .field(
                "environment",
                &format!(
                    "ProcessEnvironment({} entries; redacted)",
                    self.environment.len()
                ),
            )
            .field("home_dir", &self.home_dir)
            .field("working_directory", &self.working_directory)
            .field("initialize_timeout", &self.initialize_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("codex_arguments", &self.codex_arguments)
            .finish()
    }
}

impl CodexCLIUsageFetcher {
    pub fn new(
        environment: BTreeMap<String, String>,
        home_dir: Option<PathBuf>,
        working_directory: PathBuf,
    ) -> Self {
        let process_registry = Arc::new(ProcessRegistry::new());
        let core_launch_gate = Arc::new(LaunchGate::default());
        let cli_session =
            CodexCLISession::live(Arc::clone(&process_registry), Arc::clone(&core_launch_gate));
        Self {
            environment,
            home_dir,
            state_dir: None,
            working_directory,
            initialize_timeout: Duration::from_secs(8),
            request_timeout: Duration::from_secs(3),
            codex_arguments: DEFAULT_CODEX_APP_SERVER_ARGS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            resolved_executable: None,
            process_registry,
            core_launch_gate,
            cli_session,
            cli_launch_gate: Arc::new(CodexCLILaunchGate::new()),
            interaction: FetchInteraction::UserInitiated,
            cancel: CancellationToken::new(),
        }
    }

    pub fn with_call(mut self, interaction: FetchInteraction, cancel: CancellationToken) -> Self {
        self.interaction = interaction;
        self.cancel = cancel;
        self
    }

    pub fn with_timeouts(mut self, init_timeout: Duration, req_timeout: Duration) -> Self {
        self.initialize_timeout = init_timeout;
        self.request_timeout = req_timeout;
        self
    }

    pub fn with_arguments(mut self, args: Vec<String>) -> Self {
        self.codex_arguments = args;
        self
    }

    pub fn with_executable(mut self, executable: AbsolutePath) -> Self {
        self.resolved_executable = Some(executable);
        self
    }

    pub fn with_state_directory(mut self, state_directory: impl AsRef<Path>) -> Self {
        let dir = state_directory.as_ref().to_path_buf();
        self.state_dir = Some(dir.clone());
        self.cli_session = self.cli_session.with_state_directory(Ok(dir));
        self
    }

    pub fn with_process_registry(mut self, registry: Arc<ProcessRegistry>) -> Self {
        self.cli_session = self
            .cli_session
            .with_process_registry(Arc::clone(&registry));
        self.process_registry = registry;
        self
    }

    pub fn with_core_launch_gate(mut self, gate: Arc<LaunchGate>) -> Self {
        self.cli_session = self.cli_session.with_launch_gate(Arc::clone(&gate));
        self.core_launch_gate = gate;
        self
    }

    pub fn with_process_broker(
        mut self,
        registry: Arc<ProcessRegistry>,
        gate: Arc<LaunchGate>,
    ) -> Self {
        self.cli_session = self
            .cli_session
            .with_process_broker(Arc::clone(&registry), Arc::clone(&gate));
        self.process_registry = registry;
        self.core_launch_gate = gate;
        self
    }

    pub fn with_cli_session(mut self, session: CodexCLISession) -> Self {
        self.process_registry = Arc::clone(session.process_registry());
        self.core_launch_gate = Arc::clone(session.launch_gate());
        self.cli_session = session;
        self
    }

    pub fn resolve_binary(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Option<AbsolutePath> {
        let proc_env = ProcessEnv::from_iter(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        let home = home_dir
            .map(Path::to_path_buf)
            .or_else(|| env.get("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/nonexistent"));
        let inputs = ResolverInputs::new(&proc_env, &home, &RealResolverFs);
        resolve_codex_for_rpc(&inputs, "codex", || None)
            .ok()
            .map(|r| r.executable)
    }

    pub async fn detect_cli_version(&self, call: Option<&BrokerCall>) -> Option<String> {
        let executable = match &self.resolved_executable {
            Some(exec) => exec.clone(),
            None => Self::resolve_binary(&self.environment, self.home_dir.as_deref())?,
        };
        let proc_env = ProcessEnv::from_allowlist(
            self.environment.iter().map(|(k, v)| (k.clone(), v.clone())),
        );
        let candidates: &[&[&str]] = &[&["--version"], &["version"], &["-v"]];
        for candidate in candidates {
            let call = call.cloned().unwrap_or_else(|| BrokerCall {
                interaction: self.interaction,
                cancel: self.cancel.clone(),
                request_id: "codex.version".to_owned(),
            });
            if call.cancel.is_cancelled() {
                return None;
            }
            let args: Vec<OsString> = candidate.iter().map(OsString::from).collect();
            let mut cmd = Command::new(executable.clone(), args)
                .with_env(proc_env.clone())
                .timeout(Duration::from_secs(2));
            cmd.set_cwd(Some(self.working_directory.clone()));
            let Ok(output) = crate::brokers::process::run(
                call,
                cmd,
                &self.process_registry,
                &self.core_launch_gate,
            )
            .await
            else {
                continue;
            };
            if !output.status.success() {
                continue;
            }
            if let Some(first_line) = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(first_line.to_owned());
            }
        }
        None
    }

    pub async fn load_latest_usage(
        &self,
        now: Timestamp,
    ) -> Result<UsageSnapshot, CodexCLIFetchError> {
        let snapshot = self.load_latest_cli_account_snapshot(now).await?;
        snapshot
            .usage
            .ok_or(CodexCLIFetchError::Usage(UsageError::NoRateLimitsFound))
    }

    pub async fn load_latest_credits(
        &self,
        now: Timestamp,
    ) -> Result<CreditsSnapshot, CodexCLIFetchError> {
        let snapshot = self.load_latest_cli_account_snapshot(now).await?;
        snapshot
            .credits
            .ok_or(CodexCLIFetchError::Usage(UsageError::NoRateLimitsFound))
    }

    #[must_use]
    pub fn status_probe(&self, keep_cli_sessions_alive: bool) -> CodexStatusProbe {
        let session = self.cli_session.clone().with_process_broker(
            Arc::clone(&self.process_registry),
            Arc::clone(&self.core_launch_gate),
        );
        let mut probe = CodexStatusProbe::new()
            .with_environment(self.environment.clone())
            .with_home_dir(self.home_dir.clone())
            .with_timeout(self.initialize_timeout)
            .with_keep_cli_sessions_alive(keep_cli_sessions_alive)
            .with_session(session)
            .with_process_registry(Arc::clone(&self.process_registry))
            .with_core_launch_gate(Arc::clone(&self.core_launch_gate))
            .with_cli_launch_gate(Arc::clone(&self.cli_launch_gate));
        if let Some(state_dir) = &self.state_dir {
            probe = probe.with_state_directory(state_dir);
        }
        if let Some(exec) = &self.resolved_executable {
            probe = probe.with_executable(exec.clone());
        }
        probe
    }

    pub async fn load_latest_status(
        &self,
        now: Timestamp,
        keep_cli_sessions_alive: bool,
    ) -> Result<CodexStatusSnapshot, CodexStatusProbeError> {
        let call = BrokerCall {
            interaction: self.interaction,
            cancel: self.cancel.clone(),
            request_id: "codex.pty.status".to_owned(),
        };
        self.status_probe(keep_cli_sessions_alive)
            .fetch_with_call(now, call)
            .await
    }

    pub async fn load_latest_cli_account_snapshot(
        &self,
        now: Timestamp,
    ) -> Result<CodexCLIAccountSnapshot, CodexCLIFetchError> {
        self.load_latest_cli_account_snapshot_with_call(now, None)
            .await
    }

    pub async fn load_latest_cli_account_snapshot_with_call(
        &self,
        now: Timestamp,
        call: Option<BrokerCall>,
    ) -> Result<CodexCLIAccountSnapshot, CodexCLIFetchError> {
        let call = call.unwrap_or_else(|| BrokerCall {
            interaction: self.interaction,
            cancel: self.cancel.clone(),
            request_id: "codex.cli".to_owned(),
        });
        if call.cancel.is_cancelled() {
            return Err(CodexCLIFetchError::Cancelled);
        }

        let executable = match &self.resolved_executable {
            Some(exec) => exec.clone(),
            None => Self::resolve_binary(&self.environment, self.home_dir.as_deref())
                .ok_or(CodexStatusProbeError::CodexNotInstalled)?,
        };
        let binary_str = executable.as_path().to_string_lossy().to_string();
        if let Some(msg) =
            self.cli_launch_gate
                .background_skip_message(&binary_str, now, call.interaction)
        {
            return Err(CodexCLIFetchError::Wire(RPCWireError::StartFailed(msg)));
        }

        let mut proc_env = ProcessEnv::from_allowlist(
            self.environment.iter().map(|(k, v)| (k.clone(), v.clone())),
        );
        if let Some(codex_home) = self.environment.get("CODEX_HOME") {
            proc_env = proc_env.with("CODEX_HOME", codex_home);
        }

        let args: Vec<OsString> = self.codex_arguments.iter().map(OsString::from).collect();
        let mut cmd = Command::new(executable, args).with_env(proc_env);
        cmd.set_cwd(Some(self.working_directory.clone()));
        cmd.set_reap_marker(true);

        let session = match RpcSession::spawn(
            call,
            cmd,
            &self.process_registry,
            &self.core_launch_gate,
        )
        .await
        {
            Ok(s) => s,
            Err(RpcError::Cancelled) => {
                return Err(CodexCLIFetchError::Cancelled);
            }
            Err(err) => {
                let msg = err.to_string();
                let throttled = self
                    .cli_launch_gate
                    .record_launch_failure(&binary_str, &msg, now);
                return Err(CodexCLIFetchError::Wire(RPCWireError::StartFailed(
                    throttled.unwrap_or(msg),
                )));
            }
        };

        let result = self.run_rpc_sequence(&session, now).await;
        session.shutdown().await;

        match result {
            Ok(snapshot) => Ok(snapshot),
            Err(RPCWireError::Cancelled) => Err(CodexCLIFetchError::Cancelled),
            Err(wire_err) => {
                let err_text = wire_err.to_string();
                let usage = Self::recover_usage_from_rpc_error(&wire_err, now);
                let credits = Self::recover_credits_from_rpc_error(&wire_err, now);
                if usage.is_some() || credits.is_some() {
                    let identity = usage.as_ref().and_then(|u| u.identity.clone());
                    return Ok(CodexCLIAccountSnapshot {
                        usage,
                        credits,
                        identity,
                    });
                }
                if let Some(backend_err) = CodexCLIBackendRateLimitError::classify_with_env(
                    &err_text,
                    &self.environment,
                    self.home_dir.as_deref(),
                ) {
                    return Err(CodexCLIFetchError::BackendRateLimit(backend_err));
                }
                if matches!(wire_err, RPCWireError::Malformed(ref s) if s == "__no_rate_limits__") {
                    return Err(CodexCLIFetchError::Usage(UsageError::NoRateLimitsFound));
                }
                Err(CodexCLIFetchError::Wire(wire_err))
            }
        }
    }

    async fn run_rpc_sequence(
        &self,
        session: &RpcSession,
        now: Timestamp,
    ) -> Result<CodexCLIAccountSnapshot, RPCWireError> {
        let init_params = RawValue::from_string(
            r#"{"clientInfo":{"name":"ariadusage","version":"0.0.0"}}"#.into(),
        )
        .expect("valid static JSON");
        let empty_params = RawValue::from_string("{}".into()).expect("valid static JSON");

        session
            .request_with_error_message("initialize", &init_params, self.initialize_timeout)
            .await
            .map_err(|(err, msg)| map_rpc_error("initialize", err, msg))?;

        session
            .notify("initialized", &empty_params)
            .await
            .map_err(|err| map_rpc_error("initialized", err, None))?;

        let limits_raw = session
            .request_with_error_message(
                "account/rateLimits/read",
                &empty_params,
                self.request_timeout,
            )
            .await
            .map_err(|(err, msg)| map_rpc_error("account/rateLimits/read", err, msg))?;

        let limits_response: RPCRateLimitsResponse = serde_json::from_str(limits_raw.get())
            .map_err(|e| RPCWireError::Malformed(e.to_string()))?;

        let account_response: Option<RPCAccountResponse> = session
            .request("account/read", &empty_params, self.request_timeout)
            .await
            .ok()
            .and_then(|raw| serde_json::from_str(raw.get()).ok());

        Self::map_rpc_responses(&limits_response, account_response.as_ref(), now)
            .ok_or_else(|| RPCWireError::Malformed("__no_rate_limits__".to_owned()))
    }

    pub fn map_rpc_responses(
        limits_response: &RPCRateLimitsResponse,
        account_response: Option<&RPCAccountResponse>,
        now: Timestamp,
    ) -> Option<CodexCLIAccountSnapshot> {
        let limits = &limits_response.rate_limits;
        let rate_limits_plan = Self::normalized_codex_account_field(limits.plan_type.as_deref());
        let account_email = account_response
            .and_then(|a| a.account.as_ref())
            .and_then(|details| match details {
                RPCAccountDetails::ChatGPT { email, .. } => Some(email.clone()),
                RPCAccountDetails::ApiKey => None,
            });
        let account_plan = account_response
            .and_then(|a| a.account.as_ref())
            .and_then(|details| match details {
                RPCAccountDetails::ChatGPT { plan_type, .. } => Some(plan_type.clone()),
                RPCAccountDetails::ApiKey => None,
            });
        let identity = ProviderIdentitySnapshot::new(
            Some(ProviderId::new("codex").expect("valid provider id")),
            account_email,
            None,
            rate_limits_plan.clone().or(account_plan),
            None,
        );
        let credits = Self::make_credits(
            limits,
            limits_response.rate_limits_by_limit_id.as_ref(),
            now,
        );
        let should_return_unavailable_usage = credits.is_none() || rate_limits_plan.is_some();
        let usage = CodexReconciledState::from_cli(
            Self::make_window_from_rpc(limits.primary.as_ref()),
            Self::make_window_from_rpc(limits.secondary.as_ref()),
            Some(identity.clone()),
            now,
        )
        .map(|s| s.to_usage_snapshot(Confidence::Unknown))
        .or_else(|| {
            if should_return_unavailable_usage {
                Self::empty_codex_usage_snapshot_if_identified(&identity, now)
            } else {
                None
            }
        });

        if usage.is_none() && credits.is_none() {
            return None;
        }
        Some(CodexCLIAccountSnapshot {
            usage,
            credits,
            identity: Some(identity),
        })
    }

    pub fn load_account_info(&self) -> AccountInfo {
        let account = self.load_auth_backed_codex_account();
        AccountInfo::new(account.email, account.plan)
    }

    pub fn load_auth_backed_codex_account(&self) -> CodexAuthBackedAccount {
        let credentials =
            CodexOAuthCredentialsStore::load(&self.environment, self.home_dir.as_deref()).ok();
        CodexAuthBackedAccount::from_credentials(credentials.as_ref())
    }

    fn make_window_from_rpc(rpc: Option<&RPCRateLimitWindow>) -> Option<RateWindow> {
        let rpc = rpc?;
        let resets_at = rpc.resets_at.and_then(|ts| Timestamp::from_second(ts).ok());
        RateWindow::new(
            rpc.used_percent,
            rpc.window_duration_mins,
            resets_at,
            None,
            None,
            false,
        )
        .ok()
    }

    fn make_tty_window(
        percent_left: Option<i64>,
        window_minutes: i64,
        resets_at: Option<Timestamp>,
        reset_description: Option<String>,
    ) -> Option<RateWindow> {
        let percent_left = percent_left?;
        let used_percent = (100.0 - (percent_left as f64)).max(0.0);
        RateWindow::new(
            used_percent,
            Some(window_minutes),
            resets_at,
            reset_description,
            None,
            false,
        )
        .ok()
    }

    pub fn make_credits(
        limits: &RPCRateLimitSnapshot,
        rate_limits_by_limit_id: Option<&BTreeMap<String, RPCRateLimitSnapshot>>,
        updated_at: Timestamp,
    ) -> Option<CreditsSnapshot> {
        let balance = limits.credits.as_ref().map(|c| {
            c.balance
                .as_deref()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0)
        });
        let balance_was_read = limits
            .credits
            .as_ref()
            .and_then(|c| c.balance.as_deref())
            .and_then(|s| s.parse::<f64>().ok())
            .is_some();
        let credit_limit = Self::codex_credit_limit(limits, rate_limits_by_limit_id, updated_at);
        if balance.is_none() && credit_limit.is_none() {
            return None;
        }
        Some(CreditsSnapshot::with_details(
            balance.unwrap_or(0.0),
            Vec::new(),
            updated_at,
            credit_limit,
            balance_was_read,
            None,
            false,
        ))
    }

    fn codex_credit_limit(
        limits: &RPCRateLimitSnapshot,
        rate_limits_by_limit_id: Option<&BTreeMap<String, RPCRateLimitSnapshot>>,
        updated_at: Timestamp,
    ) -> Option<CodexCreditLimitSnapshot> {
        if let Some(limit) = Self::codex_credit_limit_from_snapshot(limits, updated_at) {
            return Some(limit);
        }
        if let Some(by_id) = rate_limits_by_limit_id {
            let mut candidates: Vec<&RPCRateLimitSnapshot> = by_id.values().collect();
            candidates.sort_by(|a, b| {
                let key_a = a
                    .limit_name
                    .as_deref()
                    .or(a.limit_id.as_deref())
                    .unwrap_or("");
                let key_b = b
                    .limit_name
                    .as_deref()
                    .or(b.limit_id.as_deref())
                    .unwrap_or("");
                key_a.cmp(key_b)
            });
            for candidate in candidates {
                if let Some(limit) = Self::codex_credit_limit_from_snapshot(candidate, updated_at) {
                    return Some(limit);
                }
            }
        }
        None
    }

    fn codex_credit_limit_from_snapshot(
        snapshot: &RPCRateLimitSnapshot,
        updated_at: Timestamp,
    ) -> Option<CodexCreditLimitSnapshot> {
        let individual_limit = snapshot.individual_limit.as_ref()?;
        let limit = individual_limit.limit?;
        if !limit.is_finite() || limit <= 0.0 {
            return None;
        }
        let used = if let Some(used) = individual_limit.used {
            used
        } else if let Some(remaining_pct) = individual_limit.remaining_percent {
            limit * (100.0 - remaining_pct).clamp(0.0, 100.0) / 100.0
        } else {
            0.0
        };
        let remaining_percent = individual_limit
            .remaining_percent
            .unwrap_or_else(|| (100.0 - (used / limit * 100.0)).clamp(0.0, 100.0));
        let resets_at = individual_limit
            .resets_at
            .filter(|&v| v > 0)
            .and_then(|v| Timestamp::from_second(v).ok());
        let title = snapshot
            .limit_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Monthly credit limit");

        Some(CodexCreditLimitSnapshot::new(
            title,
            used,
            limit,
            remaining_percent,
            resets_at,
            updated_at,
        ))
    }

    fn empty_codex_usage_snapshot_if_identified(
        identity: &ProviderIdentitySnapshot,
        now: Timestamp,
    ) -> Option<UsageSnapshot> {
        if identity.account_email.is_none() && identity.login_method.is_none() {
            return None;
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
            now,
            Some(identity.clone()),
            Confidence::Unknown,
        )
        .ok()
    }

    pub fn recover_usage_from_rpc_error(
        error: &RPCWireError,
        now: Timestamp,
    ) -> Option<UsageSnapshot> {
        let body = Self::decode_rate_limits_error_body(error)?;
        let identity = ProviderIdentitySnapshot::new(
            Some(ProviderId::new("codex").expect("valid provider id")),
            Self::normalized_codex_account_field(body.email.as_deref()),
            None,
            Self::normalized_codex_account_field(body.plan_type.as_deref()),
            None,
        );
        let primary = body
            .rate_limit
            .as_ref()
            .and_then(|rl| rl.primary_window.as_ref())
            .and_then(WindowSnapshot::to_rate_window_unchecked);
        let secondary = body
            .rate_limit
            .as_ref()
            .and_then(|rl| rl.secondary_window.as_ref())
            .and_then(WindowSnapshot::to_rate_window_unchecked);
        let state = CodexReconciledState::from_cli(primary, secondary, Some(identity), now)?;
        if body
            .rate_limit
            .as_ref()
            .is_some_and(RateLimitDetails::has_window_decode_failure)
            && state.session.is_none()
        {
            return None;
        }
        Some(state.to_usage_snapshot(Confidence::Unknown))
    }

    pub fn recover_credits_from_rpc_error(
        error: &RPCWireError,
        now: Timestamp,
    ) -> Option<CreditsSnapshot> {
        let credits = Self::decode_rate_limits_error_body(error)?.credits?;
        let remaining = credits.balance?;
        Some(CreditsSnapshot::new(remaining, Vec::new(), now))
    }

    fn decode_rate_limits_error_body(error: &RPCWireError) -> Option<RPCRateLimitsErrorBody> {
        let RPCWireError::RequestFailed(message) = error else {
            return None;
        };
        let json = Self::extract_json_object_after("body=", message)?;
        serde_json::from_str::<RPCRateLimitsErrorBody>(json).ok()
    }

    pub fn extract_json_object_after<'a>(marker: &str, text: &'a str) -> Option<&'a str> {
        let idx = text.find(marker)?;
        let suffix = &text[idx + marker.len()..];
        let start = suffix.find('{')?;
        let slice = &suffix[start..];

        let mut depth = 0i32;
        let mut in_string = false;
        let mut is_escaped = false;

        for (offset, ch) in slice.char_indices() {
            if in_string {
                if is_escaped {
                    is_escaped = false;
                } else if ch == '\\' {
                    is_escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&slice[..=offset + ch.len_utf8() - 1]);
                    }
                }
                _ => {}
            }
        }
        None
    }

    pub fn normalized_codex_account_field(value: Option<&str>) -> Option<String> {
        let trimmed = value?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }

    pub fn map_codex_rpc_limits_for_testing(
        primary: Option<(f64, i64, Option<i64>)>,
        secondary: Option<(f64, i64, Option<i64>)>,
        plan_type: Option<&str>,
        now: Timestamp,
    ) -> Result<UsageSnapshot, UsageError> {
        let identity = ProviderIdentitySnapshot::new(
            Some(ProviderId::new("codex").expect("valid provider id")),
            None,
            None,
            Self::normalized_codex_account_field(plan_type),
            None,
        );
        let primary_win = primary.and_then(|(used, mins, reset)| {
            RateWindow::new(
                used,
                Some(mins),
                reset.and_then(|ts| Timestamp::from_second(ts).ok()),
                None,
                None,
                false,
            )
            .ok()
        });
        let secondary_win = secondary.and_then(|(used, mins, reset)| {
            RateWindow::new(
                used,
                Some(mins),
                reset.and_then(|ts| Timestamp::from_second(ts).ok()),
                None,
                None,
                false,
            )
            .ok()
        });
        if let Some(state) =
            CodexReconciledState::from_cli(primary_win, secondary_win, Some(identity.clone()), now)
        {
            return Ok(state.to_usage_snapshot(Confidence::Unknown));
        }
        if let Some(usage) = Self::empty_codex_usage_snapshot_if_identified(&identity, now) {
            return Ok(usage);
        }
        Err(UsageError::NoRateLimitsFound)
    }

    pub fn map_codex_status_for_testing(
        status: &CodexStatusSnapshot,
        now: Timestamp,
    ) -> Result<UsageSnapshot, UsageError> {
        let primary = Self::make_tty_window(
            status.five_hour_percent_left,
            300,
            status.five_hour_resets_at,
            status.five_hour_reset_description.clone(),
        );
        let secondary = Self::make_tty_window(
            status.weekly_percent_left,
            10_080,
            status.weekly_resets_at,
            status.weekly_reset_description.clone(),
        );
        let state = CodexReconciledState::from_cli(primary, secondary, None, now)
            .ok_or(UsageError::NoRateLimitsFound)?;
        Ok(state.to_usage_snapshot(Confidence::Unknown))
    }

    pub fn recover_codex_rpc_usage_from_error_for_testing(
        message: &str,
        now: Timestamp,
    ) -> Option<UsageSnapshot> {
        Self::recover_usage_from_rpc_error(
            &RPCWireError::RequestFailed(zeroize::Zeroizing::new(message.to_owned())),
            now,
        )
    }

    pub fn recover_codex_rpc_credits_from_error_for_testing(
        message: &str,
        now: Timestamp,
    ) -> Option<CreditsSnapshot> {
        Self::recover_credits_from_rpc_error(
            &RPCWireError::RequestFailed(zeroize::Zeroizing::new(message.to_owned())),
            now,
        )
    }
}

fn map_rpc_error(
    method: &str,
    err: RpcError,
    raw_msg: Option<zeroize::Zeroizing<String>>,
) -> RPCWireError {
    match err {
        RpcError::Cancelled => RPCWireError::Cancelled,
        RpcError::TimedOut => RPCWireError::Timeout {
            method: method.to_owned(),
        },
        RpcError::RequestFailed => {
            let msg =
                raw_msg.unwrap_or_else(|| zeroize::Zeroizing::new("request failed".to_owned()));
            RPCWireError::RequestFailed(msg)
        }
        RpcError::Malformed | RpcError::LineTooLong => RPCWireError::Malformed(err.to_string()),
        RpcError::Closed | RpcError::Process(_) => {
            RPCWireError::RequestFailed(zeroize::Zeroizing::new(err.to_string()))
        }
    }
}
