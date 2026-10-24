// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use super::cli_print_failure::AntigravityCLIPrintFailure;
use super::cli_session::{
    AntigravityCLIAuthenticationPrompt, AntigravityCLISession, ProcessLaunchRequest,
};
use super::descriptor::{
    ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID, AntigravitySelectedAccountGuard, AntigravityStrategyConfig,
};
use super::oauth_store::AntigravityOAuthCredentialsStore;
use super::status_probe::{
    AntigravityProcessKind, AntigravityStatusProbe, AntigravityStatusProbeError as Error,
    AntigravityStatusSnapshot, OperationDeadline, ProcessInfoResult, RequestContext,
};
use crate::brokers::call::BrokerCall;
use crate::brokers::exec_resolver::{RealResolverFs, ResolverInputs, Tool};
use crate::brokers::process::{
    AbsolutePath, Command, ProcessEnv, ProcessError, ProcessRegistry, StreamPolicy,
};
use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::{
    BoxFuture, FetchContext, FetchError, FetchResult, FetchRuntime, FetchStrategy,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_protocol::metric::SourceKind;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub type AgyVersion = (u64, u64, u64);
pub const FIRST_CSRF_GATED_VERSION: AgyVersion = (1, 2, 2);
const PRINT_OUTPUT_CAP: usize = 1_048_576;
const PRINT_TIMEOUT: Duration = Duration::from_secs(90);
pub const PRINT_ARGUMENTS: &[&str] = &[
    "-p",
    "/usage",
    "--output-format",
    "json",
    "--print-timeout",
    "90s",
];

pub struct CliProcessRequest {
    pub binary: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub directory: Option<PathBuf>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}
impl fmt::Debug for CliProcessRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CliProcessRequest")
            .field("argument_count", &self.arguments.len())
            .field("environment_count", &self.environment.len())
            .field("timeout", &self.timeout)
            .field("max_output_bytes", &self.max_output_bytes)
            .finish_non_exhaustive()
    }
}
pub struct CliProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}
impl fmt::Debug for CliProcessOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CliProcessOutput")
            .field("stdout_bytes", &self.stdout.len())
            .field("stderr_bytes", &self.stderr.len())
            .field("exit_code", &self.exit_code)
            .finish()
    }
}
pub type CliProcessRunner = Arc<
    dyn Fn(
            CliProcessRequest,
            CancellationToken,
        ) -> BoxFuture<'static, Result<CliProcessOutput, ProcessError>>
        + Send
        + Sync,
>;
pub type CliBinaryResolver = Arc<dyn Fn(&FetchContext, &Path) -> Option<PathBuf> + Send + Sync>;

#[derive(Default)]
pub struct AgyVersionResolver {
    outcome: Mutex<Option<Result<Option<AgyVersion>, Error>>>,
}
impl AgyVersionResolver {
    pub fn new() -> Self {
        Self::default()
    }
    pub async fn resolve<F, Fut>(
        &self,
        cancel: &CancellationToken,
        probe: F,
    ) -> Result<Option<AgyVersion>, Error>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Option<AgyVersion>, Error>>,
    {
        check_cancel(cancel)?;
        let mut outcome = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            outcome = self.outcome.lock() => outcome,
        };
        check_cancel(cancel)?;
        if let Some(result) = &*outcome {
            return result.clone();
        }
        let result = probe().await;
        check_cancel(cancel)?;
        if !matches!(result, Err(Error::Cancelled)) {
            *outcome = Some(result.clone());
        }
        result
    }
}
pub fn check_cancel(cancel: &CancellationToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
pub fn parse_version(output: &str) -> Option<AgyVersion> {
    let value = output.trim();
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}
pub async fn fetch_by_spawning_if_reachable<T, F, Fut>(
    version: Option<AgyVersion>,
    spawn: F,
) -> Result<T, Error>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, Error>>,
{
    if version.is_some_and(|v| v >= FIRST_CSRF_GATED_VERSION) {
        return Err(Error::ApiError(
            "agy 1.2.2 or later requires a local CSRF token".into(),
        ));
    }
    spawn().await
}
pub fn child_environment(environment: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut child = environment.clone();
    child.remove(AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY);
    child
}
pub fn broker_runner(
    call: BrokerCall,
    registry: Arc<ProcessRegistry>,
    gate: Arc<LaunchGate>,
) -> CliProcessRunner {
    Arc::new(move |request, cancel| {
        let registry = registry.clone();
        let gate = gate.clone();
        let mut call = call.clone();
        call.cancel = cancel;
        Box::pin(async move {
            let program = AbsolutePath::new(&request.binary)?;
            let mut command = Command::new(
                program,
                request.arguments.into_iter().map(Into::into).collect(),
            );
            let environment = ProcessEnv::from_allowlist(request.environment.clone()).with(
                "BROWSER",
                request
                    .environment
                    .get("BROWSER")
                    .cloned()
                    .unwrap_or_default(),
            );
            let environment = match request.environment.get("SSH_TTY") {
                Some(tty) => environment.with("SSH_TTY", tty),
                None => environment,
            };
            command
                .set_env(environment)
                .set_cwd(request.directory)
                .set_timeout(request.timeout)
                .set_reap_marker(true)
                .set_retry_text_busy(true)
                .set_stdout(StreamPolicy::Capture {
                    cap: request.max_output_bytes,
                })
                .set_stderr(StreamPolicy::Capture {
                    cap: request.max_output_bytes,
                });
            let output = crate::brokers::process::run(call, command, &registry, &gate).await?;
            Ok(CliProcessOutput {
                stdout: output.stdout.to_vec(),
                stderr: output.stderr.to_vec(),
                exit_code: output.status.code().unwrap_or(-1),
            })
        })
    })
}
async fn run_checked(
    request: CliProcessRequest,
    runner: &CliProcessRunner,
    cancel: &CancellationToken,
) -> Result<CliProcessOutput, Error> {
    check_cancel(cancel)?;
    let outcome = runner(request, cancel.clone()).await;
    check_cancel(cancel)?;
    let output = outcome.map_err(|e| AntigravityCLIPrintFailure::process_error(&e))?;
    if output.exit_code != 0 {
        return Err(AntigravityCLIPrintFailure::non_zero_exit_error(
            output.exit_code,
            &output.stderr,
        ));
    }
    Ok(output)
}
pub fn version_probe_request(
    binary: &Path,
    environment: &BTreeMap<String, String>,
    directory: Option<&Path>,
    timeout: Duration,
    max_output_bytes: usize,
) -> CliProcessRequest {
    CliProcessRequest {
        binary: binary.into(),
        arguments: vec!["--version".into()],
        environment: child_environment(environment),
        directory: directory.map(Into::into),
        timeout,
        max_output_bytes,
    }
}
pub async fn agy_version(
    mut request: CliProcessRequest,
    resolver: &AgyVersionResolver,
    runner: &CliProcessRunner,
    cancel: &CancellationToken,
) -> Result<Option<AgyVersion>, Error> {
    request.arguments = vec!["--version".into()];
    request.environment = child_environment(&request.environment);
    resolver
        .resolve(cancel, || async {
            let output = run_checked(request, runner, cancel).await?;
            Ok(parse_version(&String::from_utf8_lossy(&output.stdout)))
        })
        .await
}
pub async fn run_print_usage(
    binary: &Path,
    environment: &BTreeMap<String, String>,
    directory: &Path,
    timeout: Duration,
    resolver: &AgyVersionResolver,
    runner: &CliProcessRunner,
    cancel: &CancellationToken,
) -> Result<AntigravityStatusSnapshot, Error> {
    let version = agy_version(
        version_probe_request(
            binary,
            environment,
            Some(directory),
            timeout.min(Duration::from_secs(3)),
            PRINT_OUTPUT_CAP,
        ),
        resolver,
        runner,
        cancel,
    )
    .await?;
    if !version.is_some_and(|v| v >= (1, 1, 11)) {
        return Err(Error::ParseFailed(
            "CLI usage reports require agy 1.1.11 or later".into(),
        ));
    }
    for name in ["open", "xdg-open"] {
        write_browser_stub(&directory.join(name))?;
    }
    let mut environment = child_environment(environment);
    environment.insert(
        "PATH".into(),
        format!(
            "{}:{}",
            directory.display(),
            environment
                .get("PATH")
                .map_or("/usr/bin:/bin", String::as_str)
        ),
    );
    environment.insert(
        "BROWSER".into(),
        directory.join("open").to_string_lossy().into_owned(),
    );
    let output = run_checked(
        CliProcessRequest {
            binary: binary.into(),
            arguments: PRINT_ARGUMENTS.iter().map(|s| (*s).into()).collect(),
            environment,
            directory: Some(directory.into()),
            timeout,
            max_output_bytes: PRINT_OUTPUT_CAP,
        },
        runner,
        cancel,
    )
    .await?;
    super::quota_summary::parse_cli_usage_report(&output.stdout)
}
fn write_browser_stub(path: &Path) -> Result<(), Error> {
    crate::private_file::write_private(
        path,
        b"#!/bin/sh\nexit 1\n",
        &crate::private_file::WriteHooks::default(),
    )
    .map_err(|_| Error::CliReportFailed(AntigravityCLIPrintFailure::LaunchFailed))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::CliReportFailed(AntigravityCLIPrintFailure::LaunchFailed))?;
    }
    Ok(())
}

pub async fn fetch_with_report_fallback<L, LF, R, RF, S, SF>(
    context: &FetchContext,
    legacy: L,
    report: R,
    scoped: Option<S>,
) -> Result<FetchResult, Error>
where
    L: FnOnce() -> LF,
    LF: std::future::Future<Output = Result<FetchResult, Error>>,
    R: FnOnce() -> RF,
    RF: std::future::Future<Output = Result<FetchResult, Error>>,
    S: FnOnce() -> SF,
    SF: std::future::Future<Output = Result<FetchResult, Error>>,
{
    let error = match legacy().await {
        Ok(result) => {
            if AntigravitySelectedAccountGuard::validate(&result.usage, context).is_ok() {
                return Ok(result);
            }
            Error::AccountMismatch {
                expected: AntigravitySelectedAccountGuard::selected_account_email(context),
                found: result
                    .usage
                    .identity
                    .as_ref()
                    .and_then(|i| i.account_email.clone()),
            }
        }
        Err(error) => error,
    };
    fallback_after_error(context, error, report, scoped).await
}
async fn fallback_after_error<R, RF, S, SF>(
    cx: &FetchContext,
    error: Error,
    report: R,
    scoped: Option<S>,
) -> Result<FetchResult, Error>
where
    R: FnOnce() -> RF,
    RF: std::future::Future<Output = Result<FetchResult, Error>>,
    S: FnOnce() -> SF,
    SF: std::future::Future<Output = Result<FetchResult, Error>>,
{
    check_cancel(&cx.cancel)?;
    if matches!(error, Error::Cancelled) {
        return Err(error);
    }
    if cx.source_mode == SourceMode::Auto
        && (cx.selected_token_account.is_some()
            || cx
                .env
                .contains_key(AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY))
    {
        if let Some(scoped) = scoped {
            match scoped().await {
                Ok(result) => return Ok(result),
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(_) => {
                    check_cancel(&cx.cancel)?;
                }
            }
        }
        return Err(error);
    }
    report().await
}

pub type PortsProbe =
    Arc<dyn Fn(i32, Duration) -> BoxFuture<'static, Result<Vec<u16>, Error>> + Send + Sync>;
pub type SnapshotProbe = Arc<
    dyn Fn(Vec<u16>, Duration) -> BoxFuture<'static, Result<AntigravityStatusSnapshot, Error>>
        + Send
        + Sync,
>;
pub type DrainOutput = Arc<dyn Fn() -> BoxFuture<'static, Vec<u8>> + Send + Sync>;
pub struct SnapshotWaitDependencies {
    pub poll_interval: Duration,
    pub listening_ports: PortsProbe,
    pub drain_output: DrainOutput,
    pub fetch_snapshot: SnapshotProbe,
    pub now: Arc<dyn Fn() -> Instant + Send + Sync>,
}
async fn check_authentication(deps: &SnapshotWaitDependencies) -> Result<(), Error> {
    if AntigravityCLIAuthenticationPrompt::contains(&(deps.drain_output)().await) {
        Err(Error::AuthenticationRequired)
    } else {
        Ok(())
    }
}
pub async fn wait_for_snapshot(
    pid: i32,
    deadline: Instant,
    expected_account_email: Option<&str>,
    deps: &SnapshotWaitDependencies,
    cancel: &CancellationToken,
) -> Result<AntigravityStatusSnapshot, Error> {
    let mut last_error = None;
    let mut last_port_error = None;
    while (deps.now)() < deadline {
        check_cancel(cancel)?;
        check_authentication(deps).await?;
        let remaining = deadline.saturating_duration_since((deps.now)());
        let timeout = remaining.clamp(Duration::from_millis(200), Duration::from_secs(2));
        let port_started = Instant::now();
        let ports = match (deps.listening_ports)(pid, timeout).await {
            Ok(ports) => ports,
            Err(Error::PortDiscoveryPending { code, stderr }) => {
                check_cancel(cancel)?;
                last_port_error = Some(Error::PortDiscoveryFailed { code, stderr });
                Vec::new()
            }
            Err(Error::PortDiscoveryCommandPending(error)) => {
                check_cancel(cancel)?;
                last_port_error = Some(Error::PortDiscoveryCommandFailed(error));
                Vec::new()
            }
            Err(Error::PortDetectionFailed(ref message))
                if message == "no listening ports found" =>
            {
                Vec::new()
            }
            Err(error) => {
                check_authentication(deps).await?;
                return Err(error);
            }
        };
        check_cancel(cancel)?;
        if !ports.is_empty() {
            let timeout = remaining
                .saturating_sub(port_started.elapsed())
                .clamp(Duration::from_millis(200), Duration::from_secs(2));
            let snapshot = match (deps.fetch_snapshot)(ports, timeout).await {
                Ok(snapshot) => match snapshot.to_usage_snapshot() {
                    Ok(_) => Some(snapshot),
                    Err(error) => {
                        last_error = Some(error);
                        None
                    }
                },
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) => {
                    check_authentication(deps).await?;
                    last_error = Some(error);
                    None
                }
            };
            if let Some(snapshot) = snapshot {
                check_authentication(deps).await?;
                if AntigravitySelectedAccountGuard::matches(
                    snapshot.account_email.as_deref(),
                    expected_account_email,
                ) {
                    return Ok(snapshot);
                }
                let mismatch = Error::AccountMismatch {
                    expected: expected_account_email.map(Into::into),
                    found: snapshot.account_email.clone(),
                };
                if snapshot
                    .account_email
                    .as_deref()
                    .is_some_and(|email| !email.trim().is_empty())
                {
                    return Err(mismatch);
                }
                last_error = Some(mismatch);
            }
        }
        let remaining = deadline.saturating_duration_since((deps.now)());
        if remaining.is_zero() {
            break;
        }
        let sleep = deps.poll_interval.min(remaining);
        if !sleep.is_zero() {
            tokio::select! {_ = cancel.cancelled()=>return Err(Error::Cancelled),_ = tokio::time::sleep(sleep)=>{}}
        }
    }
    check_cancel(cancel)?;
    check_authentication(deps).await?;
    Err(last_error.or(last_port_error).unwrap_or_else(|| {
        Error::PortDetectionFailed("Antigravity CLI started but no listening ports found".into())
    }))
}

#[derive(Clone)]
pub struct WarmAgyProcess {
    pub info: ProcessInfoResult,
    pub executable_path: Option<PathBuf>,
    pub owner_uid: Option<u32>,
}
impl fmt::Debug for WarmAgyProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WarmAgyProcess")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}
pub type WarmProcessProbe =
    Arc<dyn Fn(Duration) -> BoxFuture<'static, Result<Vec<WarmAgyProcess>, Error>> + Send + Sync>;
pub struct WarmAgyDependencies {
    pub process_infos: WarmProcessProbe,
    pub listening_ports: PortsProbe,
    pub fetch_snapshot: SnapshotProbe,
    pub current_uid: u32,
    pub owned_pid: Arc<dyn Fn() -> BoxFuture<'static, Option<u32>> + Send + Sync>,
    pub now: Arc<dyn Fn() -> Instant + Send + Sync>,
}
pub fn process_matches_binary(process: &WarmAgyProcess, binary: &Path) -> bool {
    let canonical = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
    let candidates = [binary, canonical.as_path()];
    if let Some(exe) = &process.executable_path {
        return exe.is_absolute() && candidates.contains(&exe.as_path());
    }
    candidates.iter().any(|path| {
        let candidate = path.to_string_lossy();
        process.info.command == candidate
            || process
                .info
                .command
                .strip_prefix(candidate.as_ref())
                .is_some_and(|tail| tail.starts_with(' '))
    })
}
pub async fn try_warm_agy_fetch(
    timeout: Duration,
    expected_binary: Option<&Path>,
    expected_account: Option<&str>,
    deps: &WarmAgyDependencies,
    cancel: &CancellationToken,
) -> Result<Option<AntigravityStatusSnapshot>, Error> {
    check_cancel(cancel)?;
    let deadline = (deps.now)() + timeout;
    let remaining = || deadline.saturating_duration_since((deps.now)());
    if remaining().is_zero() {
        return Ok(None);
    }
    let infos = match (deps.process_infos)(remaining()).await {
        Ok(infos) => infos,
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        Err(_) => return Ok(None),
    };
    check_cancel(cancel)?;
    let owned = (deps.owned_pid)().await;
    check_cancel(cancel)?;
    for process in infos {
        if Some(process.info.pid as u32) == owned
            || process.owner_uid != Some(deps.current_uid)
            || AntigravityStatusProbe::antigravity_process_kind(&process.info.command)
                != Some(AntigravityProcessKind::Cli)
        {
            continue;
        }
        if expected_binary.is_some_and(|binary| !process_matches_binary(&process, binary)) {
            continue;
        }
        if remaining().is_zero() {
            return Ok(None);
        }
        let ports = match (deps.listening_ports)(process.info.pid, remaining()).await {
            Ok(ports) => ports,
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => continue,
        };
        check_cancel(cancel)?;
        if ports.is_empty() {
            continue;
        }
        if remaining().is_zero() {
            return Ok(None);
        }
        let snapshot = match (deps.fetch_snapshot)(ports, remaining()).await {
            Ok(s) => s,
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => continue,
        };
        check_cancel(cancel)?;
        if snapshot.to_usage_snapshot().is_ok()
            && AntigravitySelectedAccountGuard::matches(
                snapshot.account_email.as_deref(),
                expected_account,
            )
        {
            return Ok(Some(snapshot));
        }
    }
    check_cancel(cancel)?;
    Ok(None)
}

// A fetch future may be dropped by the pipeline's cancellation race. Keep the
// balancing finish operation alive independently of that future.
struct ProbeLease {
    session: Option<AntigravityCLISession>,
    reset_after_fetch: bool,
}
impl ProbeLease {
    async fn finish(&mut self, success: bool, authentication_required: bool) {
        if let Some(session) = self.session.take() {
            let reset = self.reset_after_fetch || authentication_required;
            let task = tokio::spawn(async move {
                session
                    .finish_probe(success, reset, authentication_required)
                    .await;
            });
            let _ = task.await;
        }
    }
}
impl Drop for ProbeLease {
    fn drop(&mut self) {
        if let Some(session) = self.session.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let reset = self.reset_after_fetch;
            runtime.spawn(async move {
                session.finish_probe(false, reset, false).await;
            });
        }
    }
}

#[cfg(feature = "test-hooks")]
pub type ManagedSpawnFetch = Arc<
    dyn Fn(PathBuf, Option<String>) -> BoxFuture<'static, Result<FetchResult, Error>> + Send + Sync,
>;

pub struct CliRuntime {
    pub registry: Arc<ProcessRegistry>,
    pub gate: Arc<LaunchGate>,
    pub session: AntigravityCLISession,
    pub runner: Option<CliProcessRunner>,
    #[cfg(feature = "test-hooks")]
    pub spawn_fetch: Option<ManagedSpawnFetch>,
    pub binary_resolver: Option<CliBinaryResolver>,
    pub warm_dependencies: Option<Arc<WarmAgyDependencies>>,
    pub persists_cli_sessions: bool,
    pub idle_window: Option<Duration>,
}
impl Default for CliRuntime {
    fn default() -> Self {
        let registry = Arc::new(ProcessRegistry::new());
        let gate = Arc::new(LaunchGate::default());
        Self {
            session: AntigravityCLISession::live(registry.clone(), gate.clone()),
            registry,
            gate,
            runner: None,
            #[cfg(feature = "test-hooks")]
            spawn_fetch: None,
            binary_resolver: None,
            warm_dependencies: None,
            persists_cli_sessions: false,
            idle_window: None,
        }
    }
}
pub struct AntigravityCLIHTTPSFetchStrategy {
    config: AntigravityStrategyConfig,
}
impl AntigravityCLIHTTPSFetchStrategy {
    pub const SOURCE_LABEL: &'static str = "cli";
    pub fn new(config: AntigravityStrategyConfig) -> Self {
        let runtime = config.cli.clone();
        Self {
            config: config.with_cli_runtime(runtime),
        }
    }
    pub fn with_runtime(mut self, runtime: Arc<CliRuntime>) -> Self {
        self.config = self.config.with_cli_runtime(runtime);
        self
    }
    pub fn resolve_binary(&self, cx: &FetchContext) -> Option<PathBuf> {
        if let Some(resolve) = &self.config.cli.binary_resolver {
            return resolve(cx, &self.config.home_directory);
        }
        let env =
            ProcessEnv::from_allowlist(cx.env.iter().map(|(k, v)| (k.as_str(), v.expose_secret())));
        let env = if let Some(path) = cx.env.get("ANTIGRAVITY_CLI_PATH") {
            env.with("ANTIGRAVITY_CLI_PATH", path.expose_secret())
        } else {
            env
        };
        crate::brokers::exec_resolver::resolve(
            Tool::Antigravity,
            &ResolverInputs::new(&env, &self.config.home_directory, &RealResolverFs),
        )
        .ok()
        .map(AbsolutePath::into_path_buf)
    }
    pub fn make_result(snapshot: &AntigravityStatusSnapshot) -> Result<FetchResult, Error> {
        Ok(FetchResult::new(
            snapshot.to_usage_snapshot()?,
            "cli",
            ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID,
            SourceKind::Cli,
        ))
    }
    pub fn should_reset_session_after_fetch(runtime: FetchRuntime, persists: bool) -> bool {
        runtime == FetchRuntime::Cli && !persists
    }
    fn call(cx: &FetchContext) -> BrokerCall {
        BrokerCall {
            interaction: cx.interaction,
            cancel: cx.cancel.clone(),
            request_id: cx.request_id.clone(),
        }
    }
    fn runner(&self, cx: &FetchContext) -> CliProcessRunner {
        self.config.cli.runner.clone().unwrap_or_else(|| {
            broker_runner(
                Self::call(cx),
                self.config.cli.registry.clone(),
                self.config.cli.gate.clone(),
            )
        })
    }
    pub async fn fetch_print_usage(
        &self,
        binary: &Path,
        environment: &BTreeMap<String, String>,
        timeout: Duration,
        resolver: &AgyVersionResolver,
        runner: &CliProcessRunner,
        cancel: &CancellationToken,
    ) -> Result<FetchResult, Error> {
        let root = tempfile::Builder::new()
            .prefix("ariadusage-agy-usage-")
            .tempdir()
            .map_err(|_| Error::CliReportFailed(AntigravityCLIPrintFailure::LaunchFailed))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(|_| Error::CliReportFailed(AntigravityCLIPrintFailure::LaunchFailed))?;
        }
        let snapshot = run_print_usage(
            binary,
            environment,
            root.path(),
            timeout,
            resolver,
            runner,
            cancel,
        )
        .await?;
        Self::make_result(&snapshot)
    }
    pub async fn fetch_using_warm_session<F, Fut>(
        &self,
        binary: &Path,
        expected_account: Option<&str>,
        deps: &WarmAgyDependencies,
        cancel: &CancellationToken,
        spawn: F,
    ) -> Result<FetchResult, Error>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<FetchResult, Error>>,
    {
        if let Some(snapshot) = try_warm_agy_fetch(
            Duration::from_secs(2),
            Some(binary),
            expected_account,
            deps,
            cancel,
        )
        .await?
        {
            return Self::make_result(&snapshot);
        }
        check_cancel(cancel)?;
        spawn().await
    }
}

impl AntigravityCLIHTTPSFetchStrategy {
    fn live_warm_dependencies(&self, cx: &FetchContext) -> WarmAgyDependencies {
        let local = self.config.local_probe.clone();
        let call = Self::call(cx);
        let session = self.config.cli.session.clone();
        let owners = Arc::new(std::sync::Mutex::new(BTreeMap::new()));
        #[cfg(unix)]
        let uid = rustix::process::getuid().as_raw();
        #[cfg(not(unix))]
        let uid = 0;
        let process_infos = {
            let local = local.clone();
            let call = call.clone();
            Arc::new(move |timeout: Duration| {
                let local = local.clone();
                let call = call.clone();
                Box::pin(async move {
                    let processes = tokio::time::timeout(
                        timeout,
                        local.same_user_processes(&call, |argv| {
                            AntigravityStatusProbe::antigravity_process_kind(&argv.command_line())
                                .is_some()
                        }),
                    )
                    .await
                    .map_err(|_| Error::TimedOut)?
                    .map_err(map_probe_error)?;
                    Ok(processes
                        .into_iter()
                        .map(|p| WarmAgyProcess {
                            executable_path: local.executable_path(p.identity),
                            owner_uid: Some(uid),
                            info: ProcessInfoResult {
                                pid: p.identity.pid,
                                csrf_token: None,
                                extension_port: None,
                                extension_csrf_token: None,
                                command: p.argv.command_line(),
                                identity: Some(p.identity),
                            },
                        })
                        .collect())
                }) as BoxFuture<'static, Result<Vec<WarmAgyProcess>, Error>>
            })
        };
        let listening_ports = {
            let local = local.clone();
            let call = call.clone();
            let owners = owners.clone();
            Arc::new(move |pid, timeout| {
                let local = local.clone();
                let call = call.clone();
                let owners = owners.clone();
                Box::pin(async move {
                    let ports = local
                        .listening_tcp_ports(&call, pid, timeout)
                        .await
                        .map_err(map_probe_error)?;
                    if let Some(identity) = local.owned_process_identity(pid) {
                        let mut owners = owners
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        for port in &ports {
                            owners.insert(*port, identity);
                        }
                    }
                    Ok(ports)
                }) as BoxFuture<'static, Result<Vec<u16>, Error>>
            })
        };
        let fetch_snapshot = {
            let config = self.config.clone();
            let cx = cx.clone();
            Arc::new(move |ports, timeout| {
                let config = config.clone();
                let cx = cx.clone();
                let owners = owners
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                Box::pin(async move { fetch_cli_ports(&config, &cx, ports, timeout, owners).await })
                    as BoxFuture<'static, Result<AntigravityStatusSnapshot, Error>>
            })
        };
        WarmAgyDependencies {
            process_infos,
            listening_ports,
            fetch_snapshot,
            current_uid: uid,
            owned_pid: Arc::new(move || {
                let session = session.clone();
                Box::pin(async move { session.pid().await })
            }),
            now: Arc::new(Instant::now),
        }
    }
    async fn fetch_by_spawning(
        &self,
        cx: &FetchContext,
        binary: &Path,
        environment: &BTreeMap<String, String>,
        expected_account: Option<&str>,
    ) -> Result<FetchResult, Error> {
        check_cancel(&cx.cancel)?;
        #[cfg(feature = "test-hooks")]
        if let Some(fetch) = &self.config.cli.spawn_fetch {
            return fetch(binary.to_path_buf(), expected_account.map(str::to_owned)).await;
        }
        let session = &self.config.cli.session;
        let request = ProcessLaunchRequest {
            binary: binary.into(),
            call: Self::call(cx),
            environment: ProcessEnv::from_allowlist(environment.clone()),
            timeout: Duration::from_secs(15),
        };
        let pid = session
            .begin_probe_with_request(request, self.config.cli.idle_window)
            .await
            .map_err(|_| Error::CliReportFailed(AntigravityCLIPrintFailure::LaunchFailed))?;
        let mut lease = ProbeLease {
            session: Some(session.clone()),
            reset_after_fetch: Self::should_reset_session_after_fetch(
                cx.runtime,
                self.config.cli.persists_cli_sessions,
            ),
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        let local = self.config.local_probe.clone();
        let call = Self::call(cx);
        let config = self.config.clone();
        let context = cx.clone();
        let identity = local.owned_process_identity(pid as i32);
        let deps = SnapshotWaitDependencies {
            poll_interval: Duration::from_millis(200),
            listening_ports: Arc::new(move |pid, timeout| {
                let local = local.clone();
                let call = call.clone();
                Box::pin(async move {
                    local
                        .listening_tcp_ports(&call, pid, timeout)
                        .await
                        .map_err(map_probe_error)
                })
            }),
            drain_output: {
                let session = session.clone();
                Arc::new(move || {
                    let session = session.clone();
                    Box::pin(async move { session.drain_output().await })
                })
            },
            fetch_snapshot: Arc::new(move |ports, timeout| {
                let config = config.clone();
                let cx = context.clone();
                let owners = ports
                    .iter()
                    .filter_map(|p| identity.map(|id| (*p, id)))
                    .collect();
                Box::pin(async move {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    fetch_cli_ports(&config, &cx, ports, timeout.min(remaining), owners).await
                })
            }),
            now: Arc::new(Instant::now),
        };
        let outcome = wait_for_snapshot(pid as i32, deadline, expected_account, &deps, &cx.cancel)
            .await
            .and_then(|snapshot| Self::make_result(&snapshot));
        let auth = matches!(outcome, Err(Error::AuthenticationRequired));
        lease.finish(outcome.is_ok(), auth).await;
        outcome
    }
    #[cfg(target_os = "macos")]
    async fn fetch_scoped_print_usage(
        &self,
        cx: &FetchContext,
        binary: &Path,
        resolver: &AgyVersionResolver,
        runner: &CliProcessRunner,
    ) -> Result<FetchResult, Error> {
        use super::scoped_print_fetch::{
            ANTIGRAVITY_USERINFO_URL, AntigravityScopedPrintError, ScopedUserInfoError,
            fetch_scoped_print_snapshot,
        };
        use crate::brokers::net::{DeclaredOrigins, NetError, NetRequest};
        use ariadusage_core::pipeline::TransportClass;
        use ariadusage_protocol::secret::SecretString;
        let call = Self::call(cx);
        let snapshot = fetch_scoped_print_snapshot(
            &cx.env,
            PRINT_TIMEOUT,
            &cx.cancel,
            |invocation| async move {
                run_print_usage(
                    binary,
                    &invocation.environment,
                    &invocation.home,
                    invocation.timeout,
                    resolver,
                    runner,
                    &cx.cancel,
                )
                .await
            },
            |request| {
                let net = self.config.net.clone();
                let call = call.clone();
                async move {
                    let url = reqwest::Url::parse(ANTIGRAVITY_USERINFO_URL)
                        .map_err(|_| ScopedUserInfoError::Failed)?;
                    let origins = DeclaredOrigins::from_urls([url.clone()])
                        .map_err(|_| ScopedUserInfoError::Failed)?;
                    let mut net_request = NetRequest::new(reqwest::Method::GET, url);
                    net_request.credentials.push((
                        reqwest::header::AUTHORIZATION,
                        SecretString::new(format!(
                            "Bearer {}",
                            request.access_token.expose_secret()
                        )),
                    ));
                    net_request.timeout = Some(request.timeout);
                    net_request.body_cap = Some(PRINT_OUTPUT_CAP);
                    let response = net
                        .send_response(&call, &origins, net_request)
                        .await
                        .map_err(|error| match error {
                            NetError::Cancelled
                            | NetError::Transport(TransportClass::Cancelled) => {
                                ScopedUserInfoError::TransportCancelled
                            }
                            _ => ScopedUserInfoError::Failed,
                        })?;
                    if response.status != reqwest::StatusCode::OK {
                        return Ok(None);
                    }
                    Ok(serde_json::from_slice::<serde_json::Value>(&response.body)
                        .ok()
                        .and_then(|value| value.get("email")?.as_str().map(str::to_owned)))
                }
            },
            cx.selected_token_account.as_ref()
                .and(self.config.credentials_update_handler.as_ref())
                .map(|handler| {
                    move |credentials| async move {
                        handler(credentials).await.map_err(|error| {
                            if error.is_cancelled() {
                                ScopedUserInfoError::Cancelled
                            } else {
                                ScopedUserInfoError::Failed
                            }
                        })
                    }
                }),
        )
        .await
        .map_err(|error| {
            match error {
                AntigravityScopedPrintError::Cancelled => Error::Cancelled,
                AntigravityScopedPrintError::Print(error) => error,
                AntigravityScopedPrintError::AccountMismatch { expected, found } => {
                    Error::AccountMismatch {
                        expected: Some(expected),
                        found,
                    }
                }
                _ => Error::ParseFailed(
                    "Antigravity scoped credentials could not be verified against the selected account."
                        .into(),
                ),
            }
        })?;
        Self::make_result(&snapshot)
    }
    pub async fn fetch_cli(&self, cx: &FetchContext) -> Result<FetchResult, Error> {
        check_cancel(&cx.cancel)?;
        let cx = AntigravitySelectedAccountGuard::resolve_context(
            cx,
            self.config.selected_token_override.as_deref(),
        );
        let binary = self.resolve_binary(&cx).ok_or(Error::NotRunning)?;
        let environment = cx
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.expose_secret().to_owned()))
            .collect::<BTreeMap<_, _>>();
        let expected = if cx.source_mode == SourceMode::Auto && cx.selected_token_account.is_some()
        {
            AntigravitySelectedAccountGuard::selected_account_email(&cx)
        } else {
            None
        };
        let resolver = AgyVersionResolver::new();
        let runner = self.runner(&cx);
        let deps = self
            .config
            .cli
            .warm_dependencies
            .clone()
            .unwrap_or_else(|| Arc::new(self.live_warm_dependencies(&cx)));
        let legacy = || async {
            self.fetch_using_warm_session(
                &binary,
                expected.as_deref(),
                &deps,
                &cx.cancel,
                || async {
                    let version = agy_version(
                        version_probe_request(
                            &binary,
                            &environment,
                            None,
                            Duration::from_secs(3),
                            4096,
                        ),
                        &resolver,
                        &runner,
                        &cx.cancel,
                    )
                    .await
                    .ok()
                    .flatten();
                    check_cancel(&cx.cancel)?;
                    fetch_by_spawning_if_reachable(version, || {
                        self.fetch_by_spawning(&cx, &binary, &environment, expected.as_deref())
                    })
                    .await
                },
            )
            .await
        };
        let report = || {
            self.fetch_print_usage(
                &binary,
                &environment,
                PRINT_TIMEOUT,
                &resolver,
                &runner,
                &cx.cancel,
            )
        };
        #[cfg(target_os = "macos")]
        {
            fetch_with_report_fallback(
                &cx,
                legacy,
                report,
                Some(|| self.fetch_scoped_print_usage(&cx, &binary, &resolver, &runner)),
            )
            .await
        }
        #[cfg(not(target_os = "macos"))]
        {
            fetch_with_report_fallback(
                &cx,
                legacy,
                report,
                None::<fn() -> std::future::Ready<Result<FetchResult, Error>>>,
            )
            .await
        }
    }
}
impl FetchStrategy for AntigravityCLIHTTPSFetchStrategy {
    fn id(&self) -> &'static str {
        ANTIGRAVITY_CLI_HTTPS_STRATEGY_ID
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Cli
    }
    fn is_available<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move { self.resolve_binary(cx).is_some() })
    }
    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            self.fetch_cli(cx).await.map_err(|error| match error {
                Error::Cancelled => FetchError::Cancelled,
                other => FetchError::Classified(other.into_classified_error()),
            })
        })
    }
    fn should_fallback(&self, _error: &FetchError, cx: &FetchContext) -> bool {
        matches!(cx.source_mode, SourceMode::Auto | SourceMode::Cli)
    }
}
fn map_probe_error(error: crate::brokers::local_probe::ProbeError) -> Error {
    error.into()
}
async fn fetch_cli_ports(
    config: &AntigravityStrategyConfig,
    cx: &FetchContext,
    ports: Vec<u16>,
    timeout: Duration,
    owners: BTreeMap<u16, crate::brokers::process::ProcessIdentity>,
) -> Result<AntigravityStatusSnapshot, Error> {
    let endpoints = AntigravityStatusProbe::cli_endpoints_for_current_platform(&ports);
    let net = config.net.clone();
    let local = config.local_probe.clone();
    let call = AntigravityCLIHTTPSFetchStrategy::call(cx);
    let send: super::status_probe::EndpointRequestSender =
        Arc::new(move |payload, endpoint, timeout| {
            let net = net.clone();
            let local = local.clone();
            let call = call.clone();
            let identity = owners.get(&endpoint.port).copied();
            Box::pin(async move {
                let Some(identity) = identity else {
                    return Err(Error::PortDetectionFailed(
                        "listener identity unavailable".into(),
                    ));
                };
                if !local
                    .listener_still_owned(&call, identity, endpoint.port)
                    .await
                    .map_err(map_probe_error)?
                {
                    return Err(Error::PortDetectionFailed(
                        "listener no longer owned by process".into(),
                    ));
                }
                AntigravityStatusProbe::send_loopback_request(
                    &net, &call, &payload, &endpoint, timeout,
                )
                .await
            })
        });
    AntigravityStatusProbe::fetch_snapshot(
        &RequestContext { endpoints, timeout },
        &OperationDeadline::new(timeout),
        send,
    )
    .await
}

#[cfg(test)]
mod cancellation_tests {
    use super::super::cli_session::{Dependencies, ProcessFuture, ProcessHandle, ProcessLauncher};
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Child(AtomicBool);
    impl ProcessHandle for Child {
        fn pid(&self) -> u32 {
            123
        }
        fn is_running(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn drain_output(&self) -> ProcessFuture<Result<Vec<u8>, String>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn close_gracefully(&self) -> ProcessFuture<()> {
            self.0.store(false, Ordering::SeqCst);
            Box::pin(async {})
        }
        fn terminate(&self) -> ProcessFuture<()> {
            self.0.store(false, Ordering::SeqCst);
            Box::pin(async {})
        }
    }
    struct Launcher(Arc<Child>);
    impl ProcessLauncher for Launcher {
        fn launch(&self, _binary: &Path) -> ProcessFuture<Result<Arc<dyn ProcessHandle>, String>> {
            let child = self.0.clone();
            Box::pin(async move { Ok(child as Arc<dyn ProcessHandle>) })
        }
    }
    #[tokio::test]
    async fn dropped_fetch_balances_probe_and_reaps_one_shot_session() {
        let child = Arc::new(Child(AtomicBool::new(true)));
        let session =
            AntigravityCLISession::new(Dependencies::new(Arc::new(Launcher(child.clone()))));
        session.begin_probe("/fixture/agy", None).await.unwrap();
        let lease = ProbeLease {
            session: Some(session.clone()),
            reset_after_fetch: true,
        };
        let task = tokio::spawn(async move {
            let _lease = lease;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while child.is_running() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!session.is_running().await);
    }
}
