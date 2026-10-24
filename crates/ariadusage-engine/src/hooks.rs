// Ported from CodexBar Sources/CodexBarCore/Hooks/HookRunner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/HookRunnerTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/HookRateLimiterTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/HookEditorValidationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIHookExecutionHelpersTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/UsageStoreUsageUpdatedHookTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Hook command execution through the [`crate::brokers::process`] broker with an injectable
//! [`HookCommandExecutor`] boundary for deterministic testing.
//!
//! Security invariants:
//! - Commands are executed with a fixed [`AbsolutePath`] and argv array (`Vec<OsString>`); a shell
//!   command string is never constructed from data.
//! - Only allowlisted general-purpose environment variables ([`FORWARDED_ENVIRONMENT_KEYS`]) plus
//!   the event's own `CODEXBAR_*` variables are passed to the child process. Provider API keys,
//!   tokens, cookies, and unallowlisted base environment variables never reach a hook.
//! - JSON stdin payloads are capped at [`MAXIMUM_PAYLOAD_BYTES`] (4096 bytes).
//! - Timeouts (`rule.timeout_seconds`) and output caps ([`DEFAULT_OUTPUT_CAP`]) are enforced by the
//!   Process broker with process-group teardown (`SIGTERM` -> `SIGKILL`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::{BoxFuture, FetchInteraction};
use ariadusage_core::refresh::{HookEvent, HookRateLimiter, HookRule, HooksConfig};
use jiff::Timestamp;
use tokio_util::sync::CancellationToken;

use crate::brokers::call::BrokerCall;
use crate::brokers::process::{
    AbsolutePath, Command, DEFAULT_OUTPUT_CAP, LaunchMode, ProcessEnv, ProcessError,
    ProcessRegistry, StdinSpec, StreamPolicy,
};

/// Maximum JSON stdin payload size in bytes (`HookRunner.maximumPayloadBytes = 4096`).
pub const MAXIMUM_PAYLOAD_BYTES: usize = 4096;

/// Allowlisted environment variable keys forwarded from the base environment to a hook process
/// (`HookRunner.forwardedEnvironmentKeys`).
pub const FORWARDED_ENVIRONMENT_KEYS: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "LC_CTYPE", "TERM", "TMPDIR",
];

/// Outcome of dispatching a [`HookEvent`] against [`HooksConfig`] and [`HookRateLimiter`]
/// (`HookRunner.DispatchOutcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    NoMatchingRules,
    RateLimited,
    /// At least one matching command was attempted, including failed launches.
    Attempted,
}

/// Errors returned when preparing or executing a single hook rule (`HookRunnerError` + `SubprocessRunnerError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HookRunnerError {
    #[error("Hook event payload exceeds {MAXIMUM_PAYLOAD_BYTES} bytes.")]
    PayloadTooLarge,
    #[error("executable not found")]
    BinaryNotFound,
    #[error("launch failed")]
    LaunchFailed,
    #[error("timed out")]
    TimedOut,
    #[error("output too large")]
    OutputTooLarge,
    #[error("exit {0}")]
    NonZeroExit(i32),
}

impl HookRunnerError {
    /// Coarse, redacted failure summary matching `HookRunner.failureSummary(_:)`.
    /// Never includes child stderr or payload contents.
    pub fn failure_summary(&self) -> String {
        match self {
            Self::PayloadTooLarge => "payload too large".to_string(),
            Self::BinaryNotFound => "executable not found".to_string(),
            Self::LaunchFailed => "launch failed".to_string(),
            Self::TimedOut => "timed out".to_string(),
            Self::OutputTooLarge => "output too large".to_string(),
            Self::NonZeroExit(code) => format!("exit {code}"),
        }
    }
}

/// Validated hook execution request passed to a [`HookCommandExecutor`].
#[derive(Debug, Clone, PartialEq)]
pub struct HookExecutionRequest {
    pub executable: String,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub payload: Vec<u8>,
    pub timeout_seconds: f64,
    pub label: String,
}

/// Captured output from a completed hook process.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HookExecutionOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Boundary for executing a validated [`HookExecutionRequest`].
///
/// Production code uses [`ProcessBrokerHookExecutor`]; tests inject [`FakeHookCommandExecutor`]
/// so CI never runs an uninjected real hook binary.
pub trait HookCommandExecutor: Send + Sync {
    fn execute<'a>(
        &'a self,
        request: HookExecutionRequest,
    ) -> BoxFuture<'a, Result<HookExecutionOutput, HookRunnerError>>;
}

/// Production [`HookCommandExecutor`] backed by [`crate::brokers::process::run`].
#[derive(Debug, Default)]
pub struct ProcessBrokerHookExecutor {
    registry: ProcessRegistry,
    gate: LaunchGate,
}

impl ProcessBrokerHookExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a [`crate::brokers::process::Command`] from `request`, validating that `executable`
    /// is an existing absolute executable file and attaching the allowlisted environment, bounded
    /// stdin payload, timeout, and output caps.
    pub fn build_command(request: &HookExecutionRequest) -> Result<Command, HookRunnerError> {
        let path = Path::new(&request.executable);
        if !path.is_absolute() {
            return Err(HookRunnerError::LaunchFailed);
        }
        let program = AbsolutePath::new(path).map_err(|_| HookRunnerError::BinaryNotFound)?;
        let args: Vec<OsString> = request.arguments.iter().map(OsString::from).collect();

        let mut env = ProcessEnv::empty();
        for (k, v) in &request.environment {
            env = env.with(k.clone(), v.clone());
        }

        let timeout_secs = if request.timeout_seconds.is_finite() && request.timeout_seconds >= 1.0
        {
            request.timeout_seconds
        } else {
            30.0
        };
        let mut cmd = Command::new(program, args)
            .with_env(env)
            .timeout(Duration::from_secs_f64(timeout_secs));
        cmd.set_stdin(StdinSpec::Bytes(request.payload.clone()));
        cmd.set_stdout(StreamPolicy::Capture {
            cap: DEFAULT_OUTPUT_CAP,
        });
        cmd.set_stderr(StreamPolicy::Capture {
            cap: DEFAULT_OUTPUT_CAP,
        });
        cmd.set_launch_mode(LaunchMode::Group);

        Ok(cmd)
    }
}

impl HookCommandExecutor for ProcessBrokerHookExecutor {
    fn execute<'a>(
        &'a self,
        request: HookExecutionRequest,
    ) -> BoxFuture<'a, Result<HookExecutionOutput, HookRunnerError>> {
        Box::pin(async move {
            let cmd = Self::build_command(&request)?;
            let call = BrokerCall {
                interaction: FetchInteraction::Background,
                cancel: CancellationToken::new(),
                request_id: format!("hook-{}", request.label),
            };
            let output = crate::brokers::process::run(call, cmd, &self.registry, &self.gate)
                .await
                .map_err(|err| match err {
                    ProcessError::TimedOut => HookRunnerError::TimedOut,
                    ProcessError::OutputTooLarge { .. } => HookRunnerError::OutputTooLarge,
                    _ => HookRunnerError::LaunchFailed,
                })?;

            let exit_code = output.status.code().unwrap_or(-1);
            if !output.status.success() {
                return Err(HookRunnerError::NonZeroExit(exit_code));
            }

            Ok(HookExecutionOutput {
                exit_code,
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        })
    }
}

/// Injectable test executor that records all [`HookExecutionRequest`]s and simulates common test
/// binaries (`/usr/bin/env`, `/bin/cat`, `/usr/bin/printf`, `/usr/bin/tee`, `/bin/echo`,
/// `/usr/bin/true`, `/bin/true`, and `/bin/sh` test assertions) in-process without spawning real
/// OS subprocesses.
pub struct FakeHookCommandExecutor {
    requests: Mutex<Vec<HookExecutionRequest>>,
    #[allow(clippy::type_complexity)]
    handler: Option<
        Arc<
            dyn Fn(&HookExecutionRequest) -> Result<HookExecutionOutput, HookRunnerError>
                + Send
                + Sync,
        >,
    >,
}

impl Default for FakeHookCommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FakeHookCommandExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeHookCommandExecutor")
            .field(
                "requests_count",
                &self.requests.lock().map(|r| r.len()).unwrap_or(0),
            )
            .finish()
    }
}

impl FakeHookCommandExecutor {
    pub fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            handler: None,
        }
    }

    pub fn with_handler<F>(handler: F) -> Self
    where
        F: Fn(&HookExecutionRequest) -> Result<HookExecutionOutput, HookRunnerError>
            + Send
            + Sync
            + 'static,
    {
        Self {
            requests: Mutex::new(Vec::new()),
            handler: Some(Arc::new(handler)),
        }
    }

    pub fn requests(&self) -> Vec<HookExecutionRequest> {
        self.requests.lock().expect("fake executor lock").clone()
    }

    fn simulate_builtin(
        request: &HookExecutionRequest,
    ) -> Result<HookExecutionOutput, HookRunnerError> {
        if !Path::new(&request.executable).is_absolute() {
            return Err(HookRunnerError::LaunchFailed);
        }
        match request.executable.as_str() {
            "/usr/bin/true" | "/bin/true" => Ok(HookExecutionOutput::default()),
            "/bin/echo" | "/usr/bin/echo" => Ok(HookExecutionOutput {
                exit_code: 0,
                stdout: format!("{}\n", request.arguments.join(" ")),
                stderr: String::new(),
            }),
            "/usr/bin/env" | "/bin/env" => {
                let mut lines = Vec::with_capacity(request.environment.len());
                for (k, v) in &request.environment {
                    lines.push(format!("{k}={v}"));
                }
                Ok(HookExecutionOutput {
                    exit_code: 0,
                    stdout: format!("{}\n", lines.join("\n")),
                    stderr: String::new(),
                })
            }
            "/bin/cat" | "/usr/bin/cat" => Ok(HookExecutionOutput {
                exit_code: 0,
                stdout: String::from_utf8_lossy(&request.payload).into_owned(),
                stderr: String::new(),
            }),
            "/usr/bin/printf" | "/bin/printf" => {
                let formatted = simulate_printf(&request.arguments);
                Ok(HookExecutionOutput {
                    exit_code: 0,
                    stdout: formatted,
                    stderr: String::new(),
                })
            }
            "/usr/bin/tee" | "/bin/tee" => {
                let mut append = false;
                let mut paths = Vec::new();
                for arg in &request.arguments {
                    if arg == "-a" {
                        append = true;
                    } else {
                        paths.push(arg.as_str());
                    }
                }
                for p in paths {
                    if append {
                        use std::io::Write;
                        let mut file = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(p)
                            .map_err(|_| HookRunnerError::LaunchFailed)?;
                        file.write_all(&request.payload)
                            .map_err(|_| HookRunnerError::LaunchFailed)?;
                    } else {
                        std::fs::write(p, &request.payload)
                            .map_err(|_| HookRunnerError::LaunchFailed)?;
                    }
                }
                Ok(HookExecutionOutput {
                    exit_code: 0,
                    stdout: String::from_utf8_lossy(&request.payload).into_owned(),
                    stderr: String::new(),
                })
            }
            "/bin/sh" => simulate_test_sh(request),
            other if other.starts_with("/nonexistent/") => Err(HookRunnerError::BinaryNotFound),
            other => {
                if Path::new(other).is_file() {
                    Ok(HookExecutionOutput::default())
                } else {
                    Err(HookRunnerError::BinaryNotFound)
                }
            }
        }
    }
}

fn simulate_printf(args: &[String]) -> String {
    let Some((fmt_str, rest)) = args.split_first() else {
        return String::new();
    };
    let mut out = String::new();
    let mut arg_iter = rest.iter();
    let chars: Vec<char> = fmt_str.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '%' && i + 1 < chars.len() && chars[i + 1] == 's' {
            if let Some(val) = arg_iter.next() {
                out.push_str(val);
            }
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn simulate_test_sh(
    request: &HookExecutionRequest,
) -> Result<HookExecutionOutput, HookRunnerError> {
    if request.arguments.len() >= 4 && request.arguments[0] == "-c" {
        let script = &request.arguments[1];
        let target_path = &request.arguments[3];
        if script.contains("printf x >> \"$1\"") && !script.contains("CODEXBAR_ACCOUNT") {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(target_path)
                .map_err(|_| HookRunnerError::LaunchFailed)?;
            file.write_all(b"x")
                .map_err(|_| HookRunnerError::LaunchFailed)?;
            return Ok(HookExecutionOutput::default());
        }
        if script.contains("CODEXBAR_ACCOUNT+x") {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(target_path)
                .map_err(|_| HookRunnerError::LaunchFailed)?;
            let marker = if request.environment.contains_key("CODEXBAR_ACCOUNT") {
                b"leak" as &[u8]
            } else {
                b"x" as &[u8]
            };
            file.write_all(marker)
                .map_err(|_| HookRunnerError::LaunchFailed)?;
            return Ok(HookExecutionOutput::default());
        }
        if script.contains("/bin/cat > \"$1\"") {
            std::fs::write(target_path, &request.payload)
                .map_err(|_| HookRunnerError::LaunchFailed)?;
            return Ok(HookExecutionOutput::default());
        }
    }
    Ok(HookExecutionOutput::default())
}

impl HookCommandExecutor for FakeHookCommandExecutor {
    fn execute<'a>(
        &'a self,
        request: HookExecutionRequest,
    ) -> BoxFuture<'a, Result<HookExecutionOutput, HookRunnerError>> {
        Box::pin(async move {
            self.requests
                .lock()
                .expect("fake executor lock")
                .push(request.clone());
            if let Some(handler) = &self.handler {
                return handler(&request);
            }
            Self::simulate_builtin(&request)
        })
    }
}

/// Executes hook commands for quota and provider events (`HookRunner.swift`).
pub struct HookRunner {
    executor: Arc<dyn HookCommandExecutor>,
    rate_limiter: Mutex<HookRateLimiter>,
}

impl std::fmt::Debug for HookRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookRunner").finish()
    }
}

impl HookRunner {
    /// Creates a new `HookRunner` backed by `executor` and a default 600-second [`HookRateLimiter`].
    pub fn new(executor: Arc<dyn HookCommandExecutor>) -> Self {
        Self {
            executor,
            rate_limiter: Mutex::new(HookRateLimiter::default()),
        }
    }

    /// Creates a new `HookRunner` with an explicit [`HookRateLimiter`].
    pub fn with_rate_limiter(
        executor: Arc<dyn HookCommandExecutor>,
        rate_limiter: HookRateLimiter,
    ) -> Self {
        Self {
            executor,
            rate_limiter: Mutex::new(rate_limiter),
        }
    }

    /// Creates a production `HookRunner` backed by [`ProcessBrokerHookExecutor`].
    pub fn new_process_broker() -> Self {
        Self::new(Arc::new(ProcessBrokerHookExecutor::new()))
    }

    /// Filters `base_environment` to [`FORWARDED_ENVIRONMENT_KEYS`] and overlays `event`'s
    /// `CODEXBAR_*` variables (`HookRunner.run` environment filtering).
    pub fn filter_environment(
        base_environment: &BTreeMap<String, String>,
        event: &HookEvent,
    ) -> BTreeMap<String, String> {
        let mut environment: BTreeMap<String, String> = base_environment
            .iter()
            .filter(|(k, _)| FORWARDED_ENVIRONMENT_KEYS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, value) in event.environment_variables() {
            environment.insert(key, value);
        }
        environment
    }

    /// Runs a single `rule` for `event` to completion using `executor`.
    pub async fn run_with_executor(
        executor: &dyn HookCommandExecutor,
        rule: &HookRule,
        event: &HookEvent,
        base_environment: &BTreeMap<String, String>,
    ) -> Result<HookExecutionOutput, HookRunnerError> {
        let payload = event.json_payload().map_err(|err| match err {
            ariadusage_core::refresh::HookPayloadError::PayloadTooLarge { .. } => {
                HookRunnerError::PayloadTooLarge
            }
            _ => HookRunnerError::LaunchFailed,
        })?;
        if payload.len() > MAXIMUM_PAYLOAD_BYTES {
            return Err(HookRunnerError::PayloadTooLarge);
        }
        let environment = Self::filter_environment(base_environment, event);
        let request = HookExecutionRequest {
            executable: rule.executable.clone(),
            arguments: rule.arguments.clone(),
            environment,
            payload,
            timeout_seconds: rule.timeout_seconds,
            label: format!("hook {}", event.event.as_str()),
        };
        executor.execute(request).await
    }

    /// Runs a single `rule` for `event` using this runner's configured [`HookCommandExecutor`].
    pub async fn run(
        &self,
        rule: &HookRule,
        event: &HookEvent,
        base_environment: &BTreeMap<String, String>,
    ) -> Result<HookExecutionOutput, HookRunnerError> {
        Self::run_with_executor(self.executor.as_ref(), rule, event, base_environment).await
    }

    /// Runs every enabled rule matching `event`, subject to the rate limiter (`HookRunner.dispatch`).
    ///
    /// Failures are contained per rule and never abort subsequent rules.
    pub async fn dispatch(
        &self,
        event: &HookEvent,
        config: &HooksConfig,
        rate_limit_account_discriminator: Option<&str>,
        base_environment: &BTreeMap<String, String>,
        now: Timestamp,
    ) -> DispatchOutcome {
        let rules = config.matching_rules(event);
        if rules.is_empty() {
            return DispatchOutcome::NoMatchingRules;
        }
        if event.event.is_rate_limited() {
            let allowed = self.rate_limiter.lock().expect("rate_limiter lock").allow(
                event,
                rate_limit_account_discriminator,
                now,
            );
            if !allowed {
                return DispatchOutcome::RateLimited;
            }
        }
        for rule in rules {
            let _ = self.run(&rule, event, base_environment).await;
        }
        DispatchOutcome::Attempted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ariadusage_core::config::{
        Config, ProviderConfig, ProviderEntry, TokenAccountMeta, TokenAccountsMeta,
    };
    use ariadusage_core::model::{ProviderIdentitySnapshot, RateWindow, UsageSnapshot};
    use ariadusage_core::pipeline::{
        FetchContext, FetchError, FetchResult, FetchStrategy, ProviderPlan,
    };
    use ariadusage_core::refresh::{
        HookDispatch, HookEventType, HookRateLimiter, HookRule, HooksConfig,
    };
    use ariadusage_protocol::ids::ProviderId;
    use ariadusage_protocol::metric::{Confidence, SourceKind};
    use ariadusage_protocol::usage::ProviderErrorKind;
    use tempfile::TempDir;

    use crate::brokers::net::{NetBroker, NetConfig};
    use crate::brokers::secret_store::SecretStore;
    use crate::brokers::secret_store::memory::MemoryBackend;
    use crate::config_store::ConfigStore;
    use crate::registry::ProviderRegistry;
    use crate::runtime::{EngineRuntime, RefreshOptions, TransitionSink};

    fn sample_event(
        event_type: HookEventType,
        provider: &str,
        usage_percent: Option<f64>,
        account: Option<&str>,
        window: Option<&str>,
    ) -> HookEvent {
        HookEvent {
            event: event_type,
            provider: provider.to_string(),
            account: account.map(ToOwned::to_owned),
            window: window.map(ToOwned::to_owned),
            usage_percent,
            window_minutes: None,
            used: None,
            limit: None,
            reset_at: Some(Timestamp::from_second(1_700_000_000).expect("ts")),
            secondary_usage_percent: None,
            secondary_window_minutes: None,
            secondary_reset_at: None,
            status: None,
            timestamp: Timestamp::from_second(1_700_000_100).expect("ts"),
        }
    }

    // -------------------------------------------------------------------------
    // TestsLinux/HooksTests.swift (Runner section)
    // -------------------------------------------------------------------------

    // upstream: TestsLinux/HooksTests.swift:167 runner executes command and passes event environment
    #[tokio::test]
    async fn runner_executes_command_and_passes_event_environment() {
        let executor = Arc::new(FakeHookCommandExecutor::new());
        let runner = HookRunner::new(executor);
        let rule = HookRule::new(HookEventType::QuotaReached, "/usr/bin/env");
        let event = sample_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session"),
        );
        let result = runner
            .run(&rule, &event, &BTreeMap::new())
            .await
            .expect("env hook succeeds");
        assert!(result.stdout.contains("CODEXBAR_EVENT=quota_reached"));
        assert!(result.stdout.contains("CODEXBAR_PROVIDER=codex"));
    }

    // upstream: TestsLinux/HooksTests.swift:176 runner does not forward secrets from the base environment
    #[tokio::test]
    async fn runner_does_not_forward_secrets_from_the_base_environment() {
        let executor = Arc::new(FakeHookCommandExecutor::new());
        let runner = HookRunner::new(executor);
        let rule = HookRule::new(HookEventType::QuotaReached, "/usr/bin/env");
        let mut base = BTreeMap::new();
        base.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
        base.insert(
            "UNRELATED_VARIABLE".to_string(),
            "sensitive-fixture".to_string(),
        );
        let event = sample_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session"),
        );
        let result = runner
            .run(&rule, &event, &base)
            .await
            .expect("env hook succeeds");
        assert!(result.stdout.contains("PATH=/usr/bin:/bin"));
        assert!(!result.stdout.contains("sensitive-fixture"));
        assert!(!result.stdout.contains("UNRELATED_VARIABLE"));
    }

    // upstream: TestsLinux/HooksTests.swift:186 runner throws on missing executable
    #[tokio::test]
    async fn runner_throws_on_missing_executable() {
        let executor = Arc::new(FakeHookCommandExecutor::new());
        let runner = HookRunner::new(executor);
        let rule = HookRule::new(HookEventType::QuotaReached, "/nonexistent/codexbar-hook");
        let event = sample_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session"),
        );
        let err = runner
            .run(&rule, &event, &BTreeMap::new())
            .await
            .expect_err("missing executable must fail");
        assert_eq!(err, HookRunnerError::BinaryNotFound);
    }

    // -------------------------------------------------------------------------
    // TestsLinux/HookDispatchTests.swift
    // -------------------------------------------------------------------------

    // upstream: TestsLinux/HookDispatchTests.swift:52 runner writes the complete JSON payload to stdin
    #[tokio::test]
    async fn runner_writes_the_complete_json_payload_to_stdin() {
        let executor = Arc::new(FakeHookCommandExecutor::new());
        let runner = HookRunner::new(executor);
        let original = sample_event(
            HookEventType::QuotaReached,
            "claude",
            Some(0.42),
            None,
            Some("session"),
        );
        let result = runner
            .run(
                &HookRule::new(HookEventType::QuotaReached, "/bin/cat"),
                &original,
                &BTreeMap::new(),
            )
            .await
            .expect("cat hook succeeds");
        let decoded: HookEvent = serde_json::from_str(&result.stdout).expect("decode json");
        assert_eq!(decoded, original);
        assert_eq!(
            result.stdout,
            "{\"event\":\"quota_reached\",\"provider\":\"claude\",\"resetAt\":\"2023-11-14T22:13:20Z\",\"timestamp\":\"2023-11-14T22:15:00Z\",\"usagePercent\":0.42,\"window\":\"session\"}"
        );
    }

    // upstream: TestsLinux/HookDispatchTests.swift:68 runner rejects payloads above the pipe-safe limit
    #[tokio::test]
    async fn runner_rejects_payloads_above_the_pipe_safe_limit() {
        let executor = Arc::new(FakeHookCommandExecutor::new());
        let runner = HookRunner::new(executor);
        let oversized = HookEvent {
            event: HookEventType::QuotaReached,
            provider: "codex".to_string(),
            account: Some("x".repeat(MAXIMUM_PAYLOAD_BYTES)),
            window: None,
            usage_percent: None,
            window_minutes: None,
            used: None,
            limit: None,
            reset_at: None,
            secondary_usage_percent: None,
            secondary_window_minutes: None,
            secondary_reset_at: None,
            status: None,
            timestamp: Timestamp::from_second(1_700_000_100).expect("ts"),
        };
        let err = runner
            .run(
                &HookRule::new(HookEventType::QuotaReached, "/bin/cat"),
                &oversized,
                &BTreeMap::new(),
            )
            .await
            .expect_err("oversized payload must fail");
        assert_eq!(err, HookRunnerError::PayloadTooLarge);
    }

    // upstream: TestsLinux/HookDispatchTests.swift:83 runner preserves whitespace and empty argument boundaries
    #[tokio::test]
    async fn runner_preserves_whitespace_and_empty_argument_boundaries() {
        let executor = Arc::new(FakeHookCommandExecutor::new());
        let runner = HookRunner::new(executor);
        let mut rule = HookRule::new(HookEventType::QuotaReached, "/usr/bin/printf");
        rule.arguments = vec![
            "<%s>|<%s>|<%s>".to_string(),
            "quota reached".to_string(),
            String::new(),
            "tail".to_string(),
        ];
        let event = sample_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session"),
        );
        let result = runner
            .run(&rule, &event, &BTreeMap::new())
            .await
            .expect("printf hook succeeds");
        assert_eq!(result.stdout, "<quota reached>|<>|<tail>");
    }

    // upstream: TestsLinux/HookDispatchTests.swift:94 dispatch coalesces repeated refresh failures
    #[tokio::test]
    async fn dispatch_coalesces_repeated_refresh_failures() {
        let tmp = TempDir::new().expect("tempdir");
        let output = tmp.path().join("codexbar-hook-rate-limit.json");
        let event = sample_event(HookEventType::RefreshFailed, "codex", None, None, None);
        let mut rule = HookRule::new(HookEventType::RefreshFailed, "/usr/bin/tee");
        rule.arguments = vec!["-a".to_string(), output.to_string_lossy().into_owned()];
        let config = HooksConfig::new(true, vec![rule]);
        let runner = HookRunner::with_rate_limiter(
            Arc::new(FakeHookCommandExecutor::new()),
            HookRateLimiter::new(600.0),
        );
        let now = Timestamp::from_second(1_700_000_100).expect("ts");

        runner
            .dispatch(&event, &config, None, &BTreeMap::new(), now)
            .await;
        runner
            .dispatch(&event, &config, None, &BTreeMap::new(), now)
            .await;

        let contents = std::fs::read_to_string(&output).expect("read output");
        assert_eq!(contents.matches("\"event\":\"refresh_failed\"").count(), 1);
    }

    // upstream: TestsLinux/HookDispatchTests.swift:112 dispatch contains one rule failure and continues
    #[tokio::test]
    async fn dispatch_contains_one_rule_failure_and_continues() {
        let tmp = TempDir::new().expect("tempdir");
        let output = tmp.path().join("codexbar-hook-failure.json");
        let event = sample_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session"),
        );
        let mut second = HookRule::new(HookEventType::QuotaReached, "/usr/bin/tee");
        second.arguments = vec![output.to_string_lossy().into_owned()];
        let config = HooksConfig::new(
            true,
            vec![
                HookRule::new(HookEventType::QuotaReached, "/nonexistent/codexbar-hook"),
                second,
            ],
        );
        let runner = HookRunner::new(Arc::new(FakeHookCommandExecutor::new()));
        let now = Timestamp::from_second(1_700_000_100).expect("ts");

        runner
            .dispatch(&event, &config, None, &BTreeMap::new(), now)
            .await;

        assert!(output.exists());
    }

    // upstream: TestsLinux/HookDispatchTests.swift:128 disabled dispatch never invokes a rule
    #[tokio::test]
    async fn disabled_dispatch_never_invokes_a_rule() {
        let tmp = TempDir::new().expect("tempdir");
        let output = tmp.path().join("codexbar-hook-disabled.json");
        let mut rule = HookRule::new(HookEventType::QuotaReached, "/usr/bin/tee");
        rule.arguments = vec![output.to_string_lossy().into_owned()];
        let config = HooksConfig::new(false, vec![rule]);
        let runner = HookRunner::new(Arc::new(FakeHookCommandExecutor::new()));
        let event = sample_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session"),
        );
        let now = Timestamp::from_second(1_700_000_100).expect("ts");

        runner
            .dispatch(&event, &config, None, &BTreeMap::new(), now)
            .await;

        assert!(!output.exists());
    }

    // -------------------------------------------------------------------------
    // TestsLinux/UsageUpdatedHookMappingTests.swift (Runner section)
    // -------------------------------------------------------------------------

    fn mapping_snapshot(now: Timestamp) -> UsageSnapshot {
        UsageSnapshot {
            primary: Some(
                RateWindow::new(0.0, Some(300), Some(now), None, None, false).expect("rw"),
            ),
            secondary: Some(
                RateWindow::new(40.0, Some(10080), Some(now), None, None, false).expect("rw"),
            ),
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: now,
            identity: None,
            data_confidence: Confidence::Unknown,
        }
    }

    // upstream: TestsLinux/UsageUpdatedHookMappingTests.swift:90 dispatch reports attempts without counting unmatched or throttled candidates
    #[tokio::test]
    async fn dispatch_reports_attempts_without_counting_unmatched_or_throttled_candidates() {
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let snap = mapping_snapshot(now);
        let event = HookEvent::usage_updated("codex", &snap, None, now);
        let runner = HookRunner::new(Arc::new(FakeHookCommandExecutor::new()));
        let empty_env = BTreeMap::new();

        let unmatched = runner
            .dispatch(
                &event,
                &HooksConfig::new(true, Vec::new()),
                None,
                &empty_env,
                now,
            )
            .await;
        assert_eq!(unmatched, DispatchOutcome::NoMatchingRules);

        let cfg = HooksConfig::new(
            true,
            vec![HookRule::new(HookEventType::UsageUpdated, "/usr/bin/true")],
        );
        let first = runner.dispatch(&event, &cfg, None, &empty_env, now).await;
        assert_eq!(first, DispatchOutcome::Attempted);

        let repeated = runner.dispatch(&event, &cfg, None, &empty_env, now).await;
        assert_eq!(repeated, DispatchOutcome::RateLimited);
    }

    // upstream: TestsLinux/UsageUpdatedHookMappingTests.swift:109 failed command attempts consume the throttle interval
    #[tokio::test]
    async fn failed_command_attempts_consume_the_throttle_interval() {
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let snap = mapping_snapshot(now);
        let event = HookEvent::usage_updated("codex", &snap, None, now);
        let runner = HookRunner::new(Arc::new(FakeHookCommandExecutor::new()));
        let empty_env = BTreeMap::new();
        let cfg = HooksConfig::new(
            true,
            vec![HookRule::new(
                HookEventType::UsageUpdated,
                "/nonexistent/codexbar-hook-fixture",
            )],
        );

        let first = runner.dispatch(&event, &cfg, None, &empty_env, now).await;
        let repeated = runner.dispatch(&event, &cfg, None, &empty_env, now).await;
        assert_eq!(first, DispatchOutcome::Attempted);
        assert_eq!(repeated, DispatchOutcome::RateLimited);
    }

    // -------------------------------------------------------------------------
    // Tests/CodexBarTests/UsageUpdatedHookTests.swift
    // -------------------------------------------------------------------------

    // upstream: Tests/CodexBarTests/UsageUpdatedHookTests.swift:8 repeated usage updates are rate limited
    #[tokio::test]
    async fn repeated_usage_updates_are_rate_limited() {
        let tmp = TempDir::new().expect("tempdir");
        let output = tmp.path().join("codexbar-usage-updated-rate-limit");
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let event = HookEvent {
            event: HookEventType::UsageUpdated,
            provider: "codex".to_string(),
            account: None,
            window: None,
            usage_percent: None,
            window_minutes: None,
            used: None,
            limit: None,
            reset_at: None,
            secondary_usage_percent: None,
            secondary_window_minutes: None,
            secondary_reset_at: None,
            status: None,
            timestamp: now,
        };
        let mut rule = HookRule::new(HookEventType::UsageUpdated, "/bin/sh");
        rule.provider = Some("codex".to_string());
        rule.arguments = vec![
            "-c".to_string(),
            "printf x >> \"$1\"".to_string(),
            "hook".to_string(),
            output.to_string_lossy().into_owned(),
        ];
        let config = HooksConfig::new(true, vec![rule]);
        let runner = HookRunner::new(Arc::new(FakeHookCommandExecutor::new()));

        runner
            .dispatch(&event, &config, None, &BTreeMap::new(), now)
            .await;
        runner
            .dispatch(&event, &config, None, &BTreeMap::new(), now)
            .await;

        assert_eq!(std::fs::read_to_string(&output).expect("read"), "x");
    }

    // upstream: Tests/CodexBarTests/UsageUpdatedHookTests.swift:34 privacy mode keeps usage update throttling account scoped
    #[tokio::test]
    async fn privacy_mode_keeps_usage_update_throttling_account_scoped() {
        let tmp = TempDir::new().expect("tempdir");
        let output = tmp.path().join("private-accounts");
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let codex = ProviderId::new("codex").expect("id");

        let mut rule = HookRule::new(HookEventType::UsageUpdated, "/bin/sh");
        rule.provider = Some("codex".to_string());
        rule.arguments = vec![
            "-c".to_string(),
            "if [ -z \"${CODEXBAR_ACCOUNT+x}\" ]; then printf x >> \"$1\"; else printf leak >> \"$1\"; fi".to_string(),
            "hook".to_string(),
            output.to_string_lossy().into_owned(),
        ];
        let hooks = HooksConfig::new(true, vec![rule]);
        let runner = HookRunner::new(Arc::new(FakeHookCommandExecutor::new()));

        let snap_a = UsageSnapshot {
            primary: Some(RateWindow::new(20.0, Some(300), None, None, None, false).expect("rw")),
            secondary: None,
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: now,
            identity: Some(ProviderIdentitySnapshot {
                provider_id: Some(codex.clone()),
                account_id: Some("account-a".to_string()),
                account_email: Some("a@example.com".to_string()),
                account_organization: None,
                login_method: None,
            }),
            data_confidence: Confidence::Exact,
        };
        let snap_b = UsageSnapshot {
            primary: Some(RateWindow::new(30.0, Some(300), None, None, None, false).expect("rw")),
            identity: Some(ProviderIdentitySnapshot {
                provider_id: Some(codex.clone()),
                account_id: Some("account-b".to_string()),
                account_email: Some("b@example.com".to_string()),
                account_organization: None,
                login_method: None,
            }),
            ..snap_a.clone()
        };

        for snap in [&snap_a, &snap_b] {
            let display_account =
                ariadusage_core::refresh::hook_account_display_name(&codex, snap, true);
            let disc = ariadusage_core::refresh::hook_account_discriminator(&codex, snap);
            let event = HookEvent::usage_updated("codex", snap, display_account, now);
            runner
                .dispatch(&event, &hooks, disc.as_deref(), &BTreeMap::new(), now)
                .await;
        }

        assert_eq!(std::fs::read_to_string(&output).expect("read"), "xx");
    }

    // upstream: Tests/CodexBarTests/UsageUpdatedHookTests.swift:76 published usage update preserves primary and secondary window cadence
    #[tokio::test]
    async fn published_usage_update_preserves_primary_and_secondary_window_cadence() {
        let primary_reset = Timestamp::from_second(1_800_000_000).expect("ts");
        let secondary_reset = Timestamp::from_second(1_800_500_000).expect("ts");
        let snapshot = UsageSnapshot {
            primary: Some(
                RateWindow::new(20.0, Some(300), Some(primary_reset), None, None, false)
                    .expect("rw"),
            ),
            secondary: Some(
                RateWindow::new(50.0, Some(10080), Some(secondary_reset), None, None, false)
                    .expect("rw"),
            ),
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: primary_reset,
            identity: None,
            data_confidence: Confidence::Exact,
        };
        let event = HookEvent::usage_updated("claude", &snapshot, None, primary_reset);
        let payload: serde_json::Value =
            serde_json::from_slice(&event.json_payload().expect("payload")).expect("json");
        assert!(payload.get("window").is_none());
        assert_eq!(payload["usagePercent"].as_f64(), Some(0.2));
        assert_eq!(payload["windowMinutes"].as_i64(), Some(300));
        assert_eq!(payload["secondaryUsagePercent"].as_f64(), Some(0.5));
        assert_eq!(payload["secondaryWindowMinutes"].as_i64(), Some(10080));
        assert_eq!(
            payload["resetAt"]
                .as_str()
                .and_then(|s| s.parse::<Timestamp>().ok()),
            Some(primary_reset)
        );
        assert_eq!(
            payload["secondaryResetAt"]
                .as_str()
                .and_then(|s| s.parse::<Timestamp>().ok()),
            Some(secondary_reset)
        );
    }

    // upstream: Tests/CodexBarTests/UsageUpdatedHookTests.swift:107 published usage update omits a synthetic primary window
    #[tokio::test]
    async fn published_usage_update_omits_a_synthetic_primary_window() {
        let secondary_reset = Timestamp::from_second(1_800_500_000).expect("ts");
        let snapshot = UsageSnapshot {
            primary: Some(RateWindow::new(0.0, Some(300), None, None, None, true).expect("rw")),
            secondary: Some(
                RateWindow::new(40.0, Some(10080), Some(secondary_reset), None, None, false)
                    .expect("rw"),
            ),
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: secondary_reset,
            identity: None,
            data_confidence: Confidence::Exact,
        };
        let event = HookEvent::usage_updated("claude", &snapshot, None, secondary_reset);
        let payload: serde_json::Value =
            serde_json::from_slice(&event.json_payload().expect("payload")).expect("json");
        assert!(payload.get("usagePercent").is_none());
        assert!(payload.get("resetAt").is_none());
        assert!(payload.get("windowMinutes").is_none());
        assert_eq!(payload["secondaryUsagePercent"].as_f64(), Some(0.4));
        assert_eq!(payload["secondaryWindowMinutes"].as_i64(), Some(10080));
    }

    // upstream: Tests/CodexBarTests/UsageUpdatedHookTests.swift:132 window cadence is available in the hook environment
    #[test]
    fn window_cadence_is_available_in_the_hook_environment() {
        let json = r#"{"event":"usage_updated","provider":"codex","windowMinutes":300,"secondaryUsagePercent":0.4,"secondaryWindowMinutes":10080,"secondaryResetAt":"2026-09-12T12:00:00Z","timestamp":"2026-09-08T12:00:00Z"}"#;
        let event: HookEvent = serde_json::from_str(json).expect("decode HookEvent");
        let env = event.environment_variables();
        assert_eq!(
            env.get("CODEXBAR_WINDOW_MINUTES").map(String::as_str),
            Some("300")
        );
        assert_eq!(
            env.get("CODEXBAR_SECONDARY_USAGE_PERCENT")
                .map(String::as_str),
            Some("0.4")
        );
        assert_eq!(
            env.get("CODEXBAR_SECONDARY_WINDOW_MINUTES")
                .map(String::as_str),
            Some("10080")
        );
        assert_eq!(
            env.get("CODEXBAR_SECONDARY_RESET_AT").map(String::as_str),
            Some("2026-09-12T12:00:00Z")
        );
    }

    struct StaticOutcomeStrategy {
        outcome: Arc<Mutex<Vec<Result<UsageSnapshot, FetchError>>>>,
    }

    impl FetchStrategy for StaticOutcomeStrategy {
        fn id(&self) -> &str {
            "fixture"
        }

        fn kind(&self) -> SourceKind {
            SourceKind::Cli
        }

        fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
            Box::pin(async { true })
        }

        fn fetch<'a>(
            &'a self,
            _cx: &'a FetchContext,
        ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
            Box::pin(async move {
                let mut guard = self.outcome.lock().expect("outcome lock");
                let next = if guard.is_empty() {
                    Err(FetchError::NoAvailableStrategy(
                        ProviderId::new("claude").unwrap(),
                    ))
                } else {
                    guard.remove(0)
                };
                match next {
                    Ok(usage) => Ok(FetchResult::new(
                        usage,
                        "fixture".to_string(),
                        "fixture".to_string(),
                        SourceKind::Cli,
                    )),
                    Err(err) => Err(err),
                }
            })
        }

        fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
            false
        }
    }

    struct StaticOutcomePlan {
        outcome: Arc<Mutex<Vec<Result<UsageSnapshot, FetchError>>>>,
    }

    impl ProviderPlan for StaticOutcomePlan {
        fn resolve_strategies<'a>(
            &'a self,
            _cx: &'a FetchContext,
        ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
            let strategy = StaticOutcomeStrategy {
                outcome: Arc::clone(&self.outcome),
            };
            Box::pin(async move { vec![Box::new(strategy) as Box<dyn FetchStrategy>] })
        }
    }

    struct DispatchedHookRecorder {
        dispatches: Mutex<Vec<HookDispatch>>,
    }

    impl DispatchedHookRecorder {
        fn new() -> Self {
            Self {
                dispatches: Mutex::new(Vec::new()),
            }
        }

        fn events(&self) -> Vec<HookEvent> {
            self.dispatches
                .lock()
                .expect("dispatches lock")
                .iter()
                .map(|d| d.event.clone())
                .collect()
        }
    }

    impl TransitionSink for DispatchedHookRecorder {
        fn on_hook_dispatch(&self, dispatch: &HookDispatch) {
            self.dispatches
                .lock()
                .expect("dispatches lock")
                .push(dispatch.clone());
        }

        fn on_session_quota_transition(
            &self,
            _provider: &ProviderId,
            _transition: ariadusage_core::refresh::SessionQuotaTransition,
        ) {
        }
    }

    // upstream: Tests/CodexBarTests/UsageUpdatedHookTests.swift:151 only a successful provider refresh publishes a usage update
    #[tokio::test]
    async fn only_a_successful_provider_refresh_publishes_a_usage_update() {
        let tmp = TempDir::new().expect("tempdir");
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let claude = ProviderId::new("claude").expect("id");
        let snap = UsageSnapshot {
            primary: Some(RateWindow::new(25.0, Some(300), None, None, None, false).expect("rw")),
            secondary: None,
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: now,
            identity: None,
            data_confidence: Confidence::Exact,
        };

        let mut registry = ProviderRegistry::default();
        registry.register_plan(
            claude.clone(),
            Arc::new(StaticOutcomePlan {
                outcome: Arc::new(Mutex::new(vec![
                    Ok(snap),
                    Err(FetchError::Classified(
                        ariadusage_core::pipeline::ClassifiedError::new(
                            ProviderErrorKind::NetworkFailure,
                            "network error",
                        ),
                    )),
                ])),
            }),
        );

        let config_store = ConfigStore::new(tmp.path().join("config.json"));
        config_store
            .update(|cfg| {
                let mut p = ProviderConfig::new(claude.clone());
                p.enabled = Some(true);
                *cfg = Config::new(Config::CURRENT_VERSION, vec![ProviderEntry::Typed(p)]);
            })
            .expect("enable claude");
        let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
        let net = Arc::new(NetBroker::new(NetConfig::default()).expect("net"));
        let recorder = Arc::new(DispatchedHookRecorder::new());
        let runtime = EngineRuntime::new(config_store, secret_store, net, registry)
            .with_transition_sink(Arc::clone(&recorder) as _)
            .with_hooks_config(HooksConfig::new(
                true,
                vec![HookRule::new(HookEventType::UsageUpdated, "/usr/bin/true")],
            ))
            .with_clock(Arc::new(move || now));

        let mut opts = RefreshOptions::user_initiated();
        opts.refresh_status = false;

        runtime.refresh_provider(claude.clone(), opts).await;
        assert_eq!(recorder.events().len(), 1);
        assert_eq!(recorder.events()[0].event, HookEventType::UsageUpdated);
        assert_eq!(recorder.events()[0].usage_percent, Some(0.25));

        // Second refresh fails -> must NOT emit another usage_updated
        runtime.refresh_provider(claude, opts).await;
        assert_eq!(recorder.events().len(), 1);
    }

    // upstream: Tests/CodexBarTests/UsageUpdatedHookTests.swift:187 stacked token account success publishes a usage update
    #[tokio::test]
    async fn stacked_token_account_success_publishes_a_usage_update() {
        let tmp = TempDir::new().expect("tempdir");
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let claude = ProviderId::new("claude").expect("id");
        let snap = UsageSnapshot {
            primary: Some(RateWindow::new(35.0, Some(300), None, None, None, false).expect("rw")),
            secondary: None,
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: now,
            identity: None,
            data_confidence: Confidence::Exact,
        };

        let mut registry = ProviderRegistry::default();
        registry.register_plan(
            claude.clone(),
            Arc::new(StaticOutcomePlan {
                outcome: Arc::new(Mutex::new(vec![Ok(snap)])),
            }),
        );

        let config_store = ConfigStore::new(tmp.path().join("config.json"));
        config_store
            .update(|cfg| {
                let mut p = ProviderConfig::new(claude.clone());
                p.enabled = Some(true);
                p.token_accounts = Some(TokenAccountsMeta {
                    version: 1,
                    active_index: 0,
                    accounts: vec![TokenAccountMeta {
                        id: "00000000-0000-0000-0000-000000000001".to_string(),
                        label: "First".to_string(),
                        added_at: 0.0,
                        last_used: None,
                        external_identifier: None,
                        usage_scope: None,
                        organization_id: None,
                        workspace_id: None,
                        claude_config_directory: None,
                        seat_credit_entitlement: None,
                        model_rate_limits: None,
                        extra: BTreeMap::new(),
                    }],
                });
                *cfg = Config::new(Config::CURRENT_VERSION, vec![ProviderEntry::Typed(p)]);
            })
            .expect("config update");
        let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
        let net = Arc::new(NetBroker::new(NetConfig::default()).expect("net"));
        let recorder = Arc::new(DispatchedHookRecorder::new());
        let runtime = EngineRuntime::new(config_store, secret_store, net, registry)
            .with_transition_sink(Arc::clone(&recorder) as _)
            .with_hooks_config(HooksConfig::new(
                true,
                vec![HookRule::new(HookEventType::UsageUpdated, "/usr/bin/true")],
            ))
            .with_clock(Arc::new(move || now));

        let mut opts = RefreshOptions::user_initiated();
        opts.refresh_status = false;
        runtime.refresh_provider(claude, opts).await;

        let events = recorder.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, HookEventType::UsageUpdated);
        assert_eq!(events[0].usage_percent, Some(0.35));
    }

    // upstream: Tests/CodexBarTests/UsageUpdatedHookTests.swift:210 stacked Codex account success publishes a usage update
    #[tokio::test]
    async fn stacked_codex_account_success_publishes_a_usage_update() {
        let tmp = TempDir::new().expect("tempdir");
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let codex = ProviderId::new("codex").expect("id");
        let snap = UsageSnapshot {
            primary: Some(RateWindow::new(45.0, Some(300), None, None, None, false).expect("rw")),
            secondary: None,
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: now,
            identity: Some(ProviderIdentitySnapshot {
                provider_id: Some(codex.clone()),
                account_id: Some("owner-a".to_string()),
                account_email: Some("owner-a@example.com".to_string()),
                account_organization: None,
                login_method: None,
            }),
            data_confidence: Confidence::Exact,
        };

        let mut registry = ProviderRegistry::default();
        registry.register_plan(
            codex.clone(),
            Arc::new(StaticOutcomePlan {
                outcome: Arc::new(Mutex::new(vec![Ok(snap)])),
            }),
        );

        let config_store = ConfigStore::new(tmp.path().join("config.json"));
        let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
        let net = Arc::new(NetBroker::new(NetConfig::default()).expect("net"));
        let recorder = Arc::new(DispatchedHookRecorder::new());
        let runtime = EngineRuntime::new(config_store, secret_store, net, registry)
            .with_transition_sink(Arc::clone(&recorder) as _)
            .with_hooks_config(HooksConfig::new(
                true,
                vec![HookRule::new(HookEventType::UsageUpdated, "/usr/bin/true")],
            ))
            .with_clock(Arc::new(move || now));

        let mut opts = RefreshOptions::user_initiated();
        opts.refresh_status = false;
        runtime.refresh_provider(codex, opts).await;

        let events = recorder.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, HookEventType::UsageUpdated);
        assert_eq!(events[0].usage_percent, Some(0.45));
    }

    // upstream: Tests/CodexBarTests/UsageUpdatedHookTests.swift:243 superseded provider refresh cannot publish a usage update
    #[tokio::test]
    async fn superseded_provider_refresh_cannot_publish_a_usage_update() {
        let tmp = TempDir::new().expect("tempdir");
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let claude = ProviderId::new("claude").expect("id");

        let (call1_started_tx, call1_started_rx) = tokio::sync::oneshot::channel::<()>();
        let (call1_resume_tx, call1_resume_rx) = tokio::sync::oneshot::channel::<()>();

        struct SupersededStrategy {
            call1_started: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
            call1_resume: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
            now: Timestamp,
        }

        impl FetchStrategy for SupersededStrategy {
            fn id(&self) -> &str {
                "fixture"
            }

            fn kind(&self) -> SourceKind {
                SourceKind::Cli
            }

            fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
                Box::pin(async { true })
            }

            fn fetch<'a>(
                &'a self,
                _cx: &'a FetchContext,
            ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
                Box::pin(async move {
                    let first_started = self.call1_started.lock().unwrap().take();
                    if let Some(tx) = first_started {
                        let _ = tx.send(());
                        let rx = self.call1_resume.lock().unwrap().take().unwrap();
                        let _ = rx.await;
                        let snap = UsageSnapshot {
                            primary: Some(
                                RateWindow::new(10.0, Some(300), None, None, None, false)
                                    .expect("rw"),
                            ),
                            secondary: None,
                            tertiary: None,
                            extra_rate_windows: None,
                            provider_cost: None,
                            details: Vec::new(),
                            subscription_expires_at: None,
                            subscription_renews_at: None,
                            subscription_renews_at_is_date_only: false,
                            subscription_expires_at_is_date_only: false,
                            updated_at: self.now,
                            identity: None,
                            data_confidence: Confidence::Exact,
                        };
                        Ok(FetchResult::new(
                            snap,
                            "fixture".to_string(),
                            "fixture".to_string(),
                            SourceKind::Cli,
                        ))
                    } else {
                        let snap = UsageSnapshot {
                            primary: Some(
                                RateWindow::new(80.0, Some(300), None, None, None, false)
                                    .expect("rw"),
                            ),
                            secondary: None,
                            tertiary: None,
                            extra_rate_windows: None,
                            provider_cost: None,
                            details: Vec::new(),
                            subscription_expires_at: None,
                            subscription_renews_at: None,
                            subscription_renews_at_is_date_only: false,
                            subscription_expires_at_is_date_only: false,
                            updated_at: self.now,
                            identity: None,
                            data_confidence: Confidence::Exact,
                        };
                        Ok(FetchResult::new(
                            snap,
                            "fixture".to_string(),
                            "fixture".to_string(),
                            SourceKind::Cli,
                        ))
                    }
                })
            }

            fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
                false
            }
        }

        struct SupersededPlan {
            call1_started: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
            call1_resume: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
            now: Timestamp,
        }

        impl ProviderPlan for SupersededPlan {
            fn resolve_strategies<'a>(
                &'a self,
                _cx: &'a FetchContext,
            ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
                let strategy = SupersededStrategy {
                    call1_started: Arc::clone(&self.call1_started),
                    call1_resume: Arc::clone(&self.call1_resume),
                    now: self.now,
                };
                Box::pin(async move { vec![Box::new(strategy) as Box<dyn FetchStrategy>] })
            }
        }

        let mut registry = ProviderRegistry::default();
        registry.register_plan(
            claude.clone(),
            Arc::new(SupersededPlan {
                call1_started: Arc::new(Mutex::new(Some(call1_started_tx))),
                call1_resume: Arc::new(Mutex::new(Some(call1_resume_rx))),
                now,
            }),
        );

        let config_store = ConfigStore::new(tmp.path().join("config.json"));
        config_store
            .update(|cfg| {
                let mut p = ProviderConfig::new(claude.clone());
                p.enabled = Some(true);
                *cfg = Config::new(Config::CURRENT_VERSION, vec![ProviderEntry::Typed(p)]);
            })
            .expect("enable claude");
        let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
        let net = Arc::new(NetBroker::new(NetConfig::default()).expect("net"));
        let recorder = Arc::new(DispatchedHookRecorder::new());
        let runtime = Arc::new(
            EngineRuntime::new(config_store, secret_store, net, registry)
                .with_transition_sink(Arc::clone(&recorder) as _)
                .with_hooks_config(HooksConfig::new(
                    true,
                    vec![HookRule::new(HookEventType::UsageUpdated, "/usr/bin/true")],
                ))
                .with_clock(Arc::new(move || now)),
        );

        let rt1 = Arc::clone(&runtime);
        let c1 = claude.clone();
        let older_task = tokio::spawn(async move {
            let mut opts = RefreshOptions::user_initiated();
            opts.refresh_status = false;
            opts.coalesce_if_refreshing = false;
            rt1.refresh_provider(c1, opts).await;
        });

        call1_started_rx.await.expect("call 1 started");

        let rt2 = Arc::clone(&runtime);
        let c2 = claude.clone();
        let newer_task = tokio::spawn(async move {
            let mut opts = RefreshOptions::settings_change();
            opts.refresh_status = false;
            rt2.refresh_provider(c2, opts).await;
        });

        tokio::task::yield_now().await;
        let _ = call1_resume_tx.send(());
        let _ = older_task.await;
        let _ = newer_task.await;

        let events = recorder.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usage_percent, Some(0.8));
    }
}
