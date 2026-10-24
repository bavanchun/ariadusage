// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityCLISession.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Warm `agy` lifecycle used by the Antigravity CLI HTTPS strategy.
//!
//! The session deliberately owns only lifecycle policy.  Process creation and
//! teardown are supplied by the process broker through [`ProcessLauncher`].
//! This keeps provider code independent of POSIX spawning and makes every
//! lifecycle branch testable with an injected process handle.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use crate::brokers::call::BrokerCall;
use crate::brokers::process::session_records::{ProcSessionRecordIdentity, SessionRecordIdentity};
use crate::brokers::process::{
    AbsolutePath, Command, ProcessEnv, ProcessRegistry, PtySession, PtySize,
};
use ariadusage_core::gates::launch::LaunchGate;
/// A boxed asynchronous operation used by the process broker seam.
pub type ProcessFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

/// A launched Antigravity CLI process.
pub trait ProcessHandle: Send + Sync {
    fn pid(&self) -> u32;
    fn is_running(&self) -> bool;
    fn marker(&self) -> Option<String> {
        None
    }
    fn identity(&self) -> Option<SessionProcessIdentity> {
        None
    }
    fn process_group(&self) -> Option<u32> {
        None
    }

    /// Drains output already available from the PTY without waiting.
    fn drain_output(&self) -> ProcessFuture<Result<Vec<u8>, String>>;

    /// Graceful close writes the CLI exit command before broker teardown.
    fn close_gracefully(&self) -> ProcessFuture<()>;

    /// Force close must not write interactive input.  The process broker is
    /// responsible for terminating the process group and reaping descendants.
    fn terminate(&self) -> ProcessFuture<()>;
}

/// Process creation seam. Implementations must use the process broker and an
/// absolute executable path; the session never builds a shell command.
pub trait ProcessLauncher: Send + Sync {
    fn launch(&self, binary: &Path) -> ProcessFuture<Result<Arc<dyn ProcessHandle>, String>>;

    fn launch_with(
        &self,
        request: ProcessLaunchRequest,
    ) -> ProcessFuture<Result<Arc<dyn ProcessHandle>, String>> {
        self.launch(&request.binary)
    }
}

#[derive(Clone)]
pub struct ProcessLaunchRequest {
    pub binary: PathBuf,
    pub call: BrokerCall,
    pub environment: ProcessEnv,
    pub timeout: Duration,
}

pub use crate::brokers::process::session_records::{
    FileSessionLaunchLock, FileSessionRecordStore, SessionLaunchLock, SessionRecord,
    SessionRecordStore,
};

#[derive(Clone, PartialEq)]
pub struct SessionProcessIdentity {
    pub executable_path: String,
    pub start_epoch: f64,
}

impl std::fmt::Debug for SessionProcessIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionProcessIdentity")
            .field("executable_path", &"<redacted>")
            .field("start_epoch", &"<redacted>")
            .finish()
    }
}

pub trait SessionIdentityProvider: Send + Sync {
    fn identity(&self, pid: u32) -> Option<SessionProcessIdentity>;
    fn current_pid(&self) -> u32;
    fn current_identity(&self) -> Option<SessionProcessIdentity>;
    fn terminate_record(&self, record: SessionRecord) -> ProcessFuture<()>;
}

/// Identity provider that validates `/proc` start ticks before every signal.
pub struct ProcSessionIdentityProvider;

impl SessionIdentityProvider for ProcSessionIdentityProvider {
    fn identity(&self, pid: u32) -> Option<SessionProcessIdentity> {
        ProcSessionRecordIdentity
            .identity(pid)
            .map(|(executable_path, start_epoch)| SessionProcessIdentity {
                executable_path,
                start_epoch,
            })
    }

    fn current_pid(&self) -> u32 {
        std::process::id()
    }

    fn current_identity(&self) -> Option<SessionProcessIdentity> {
        self.identity(self.current_pid())
    }

    fn terminate_record(&self, record: SessionRecord) -> ProcessFuture<()> {
        Box::pin(async move { ProcSessionRecordIdentity.terminate_record(record).await })
    }
}

/// Production launcher backed entirely by the process broker.
pub struct BrokerProcessLauncher {
    registry: Arc<ProcessRegistry>,
    gate: Arc<LaunchGate>,
}

impl BrokerProcessLauncher {
    #[must_use]
    pub fn new(registry: Arc<ProcessRegistry>, gate: Arc<LaunchGate>) -> Arc<Self> {
        Arc::new(Self { registry, gate })
    }
}

struct BrokerProcessHandle {
    session: Arc<Mutex<PtySession>>,
    pid: u32,
}

impl ProcessHandle for BrokerProcessHandle {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn is_running(&self) -> bool {
        self.session
            .try_lock()
            .map(|session| session.is_running())
            .unwrap_or(true)
    }

    fn marker(&self) -> Option<String> {
        self.session.try_lock().ok()?.marker()
    }

    fn identity(&self) -> Option<SessionProcessIdentity> {
        self.session
            .try_lock()
            .ok()?
            .process_identity()
            .map(|(executable_path, start_ticks)| SessionProcessIdentity {
                executable_path: executable_path.to_string_lossy().into_owned(),
                start_epoch: start_ticks as f64,
            })
    }

    fn process_group(&self) -> Option<u32> {
        self.session.try_lock().ok()?.process_group()
    }

    fn drain_output(&self) -> ProcessFuture<Result<Vec<u8>, String>> {
        let session = Arc::clone(&self.session);
        Box::pin(async move {
            let mut session = session.lock().await;
            session
                .drain_output()
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn close_gracefully(&self) -> ProcessFuture<()> {
        let session = Arc::clone(&self.session);
        Box::pin(async move {
            let mut session = session.lock().await;
            session.close().await;
        })
    }

    fn terminate(&self) -> ProcessFuture<()> {
        let session = Arc::clone(&self.session);
        Box::pin(async move {
            let mut session = session.lock().await;
            session.terminate().await;
        })
    }
}

impl ProcessLauncher for BrokerProcessLauncher {
    fn launch(&self, _binary: &Path) -> ProcessFuture<Result<Arc<dyn ProcessHandle>, String>> {
        Box::pin(async {
            Err("Antigravity CLI launch requires BrokerCall and ProcessEnv".to_owned())
        })
    }

    fn launch_with(
        &self,
        request: ProcessLaunchRequest,
    ) -> ProcessFuture<Result<Arc<dyn ProcessHandle>, String>> {
        let registry = Arc::clone(&self.registry);
        let gate = Arc::clone(&self.gate);
        Box::pin(async move {
            let program = AbsolutePath::new(&request.binary).map_err(|error| error.to_string())?;
            let mut command = Command::new(program, Vec::new())
                .with_env(request.environment)
                .timeout(request.timeout);
            command.set_reap_marker(true);
            let session = tokio::time::timeout(
                request.timeout,
                PtySession::spawn(
                    request.call,
                    command,
                    PtySize::new(50, 160),
                    &registry,
                    &gate,
                ),
            )
            .await
            .map_err(|_| "PTY launch timed out".to_owned())?
            .map_err(|error| error.to_string())?;
            let pid = session
                .pid()
                .ok_or_else(|| "PTY process exited before registration".to_owned())?;
            Ok(Arc::new(BrokerProcessHandle {
                session: Arc::new(Mutex::new(session)),
                pid,
            }) as Arc<dyn ProcessHandle>)
        })
    }
}

/// Clock seam used by lifecycle tests and idle teardown.
pub trait SessionClock: Send + Sync {
    fn sleep(&self, duration: Duration) -> ProcessFuture<()>;
}

struct TokioClock;

impl SessionClock for TokioClock {
    fn sleep(&self, duration: Duration) -> ProcessFuture<()> {
        Box::pin(async move { tokio::time::sleep(duration).await })
    }
}

/// Dependencies for [`AntigravityCLISession`].
pub struct Dependencies {
    pub launcher: Arc<dyn ProcessLauncher>,
    pub clock: Arc<dyn SessionClock>,
    pub idle_window: Duration,
    pub failure_relaunch_threshold: usize,
    pub record_store: Option<Arc<dyn SessionRecordStore>>,
    pub identity_provider: Option<Arc<dyn SessionIdentityProvider>>,
    pub launch_lock: Option<Arc<dyn SessionLaunchLock>>,
}

impl Dependencies {
    /// Builds production policy around an injected broker launcher.
    #[must_use]
    pub fn new(launcher: Arc<dyn ProcessLauncher>) -> Self {
        Self {
            launcher,
            clock: Arc::new(TokioClock),
            idle_window: Duration::from_secs(180),
            failure_relaunch_threshold: 2,
            record_store: None,
            identity_provider: None,
            launch_lock: None,
        }
    }

    fn with_state_directory(
        self,
        state_directory: Result<PathBuf, crate::paths::PathError>,
    ) -> Self {
        let Ok(state_directory) = state_directory else {
            return self;
        };
        let state_root = state_directory.join("antigravity");
        self.with_session_coordination(
            Arc::new(FileSessionRecordStore::new(
                state_root.join("agy-session.json"),
            )),
            Arc::new(ProcSessionIdentityProvider),
            FileSessionLaunchLock::new(state_root.join("agy-session.lock")),
        )
    }

    #[must_use]
    pub fn with_session_coordination(
        mut self,
        record_store: Arc<dyn SessionRecordStore>,
        identity_provider: Arc<dyn SessionIdentityProvider>,
        launch_lock: Arc<dyn SessionLaunchLock>,
    ) -> Self {
        self.record_store = Some(record_store);
        self.identity_provider = Some(identity_provider);
        self.launch_lock = Some(launch_lock);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResetCause {
    Deferred,
    OneShotCli,
    Unhealthy,
    Authentication,
}

impl ResetCause {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Deferred => "deferred reset",
            Self::OneShotCli => "one-shot CLI fetch",
            Self::Unhealthy => "unhealthy CLI HTTPS session",
            Self::Authentication => "authentication required",
        }
    }

    #[must_use]
    pub const fn select(
        authentication_required: bool,
        reset_after_fetch: bool,
        unhealthy: bool,
    ) -> Self {
        if authentication_required {
            Self::Authentication
        } else if unhealthy {
            Self::Unhealthy
        } else if reset_after_fetch {
            Self::OneShotCli
        } else {
            Self::Deferred
        }
    }
}

/// Matches only output proving that the CLI cannot recover credentials itself.
pub struct AntigravityCLIAuthenticationPrompt;

impl AntigravityCLIAuthenticationPrompt {
    pub const EVIDENCE: &'static [u8] = b"Select login method:";

    #[must_use]
    pub fn contains(output: &[u8]) -> bool {
        if output
            .windows(Self::EVIDENCE.len())
            .any(|w| w == Self::EVIDENCE)
        {
            return true;
        }
        let ascii: String = output
            .iter()
            .map(|byte| if *byte < 0x80 { *byte as char } else { ' ' })
            .collect();
        let lower = ascii.to_ascii_lowercase();
        let compact = lower.split_whitespace().collect::<Vec<_>>().join(" ");
        let no_separators = compact
            .chars()
            .filter(|character| !character.is_whitespace() && *character != ':')
            .collect::<String>();
        compact.contains("select login method")
            || compact.contains("you are not logged into antigravity")
            || no_separators.contains("keyringauthtimedout")
    }
}

/// Short compatibility alias for local tests and provider code.
pub type AuthenticationPrompt = AntigravityCLIAuthenticationPrompt;

struct HeldLaunchLock(Arc<dyn SessionLaunchLock>);
impl Drop for HeldLaunchLock {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct ProbeReservation {
    session: AntigravityCLISession,
    armed: bool,
}

impl ProbeReservation {
    fn new(session: &AntigravityCLISession) -> Self {
        Self {
            session: session.clone(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    async fn release(&mut self) {
        if self.armed {
            self.session.release_probe_reservation().await;
            self.armed = false;
        }
    }
}

impl Drop for ProbeReservation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let session = self.session.clone();
        handle.spawn(async move {
            session.release_probe_reservation().await;
        });
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SessionError {
    #[error("failed to launch Antigravity CLI session: {0}")]
    LaunchFailed(String),
}

struct State {
    process: Option<Arc<dyn ProcessHandle>>,
    binary: Option<PathBuf>,
    active_probes: usize,
    active_session_probes: usize,
    reset_requested: bool,
    hard_reset_requested: bool,
    pending_cause: Option<ResetCause>,
    generation: u64,
    failures: usize,
    recent_output: Vec<u8>,
    authentication_prompt_observed: bool,
    last_stop_reason: Option<&'static str>,
    idle_task: Option<JoinHandle<()>>,
    idle_window: Duration,
    record: Option<SessionRecord>,
}

impl State {
    fn new() -> Self {
        Self {
            process: None,
            binary: None,
            active_probes: 0,
            active_session_probes: 0,
            reset_requested: false,
            hard_reset_requested: false,
            pending_cause: None,
            generation: 0,
            failures: 0,
            recent_output: Vec::new(),
            authentication_prompt_observed: false,
            last_stop_reason: None,
            idle_task: None,
            idle_window: Duration::ZERO,
            record: None,
        }
    }
}

struct Inner {
    dependencies: Dependencies,
    state: Mutex<State>,
    lifecycle: Mutex<()>,
    changed: Notify,
}

/// Manages one bounded warm `agy` process.
#[derive(Clone)]
pub struct AntigravityCLISession {
    inner: Arc<Inner>,
}

impl AntigravityCLISession {
    #[must_use]
    pub fn live(registry: Arc<ProcessRegistry>, gate: Arc<LaunchGate>) -> Self {
        let launcher = BrokerProcessLauncher::new(registry, gate);
        let dependencies =
            Dependencies::new(launcher).with_state_directory(crate::paths::state_dir());
        Self::new(dependencies)
    }

    #[must_use]
    pub fn new(dependencies: Dependencies) -> Self {
        let mut state = State::new();
        state.idle_window = dependencies.idle_window;
        Self {
            inner: Arc::new(Inner {
                dependencies,
                state: Mutex::new(state),
                lifecycle: Mutex::new(()),
                changed: Notify::new(),
            }),
        }
    }

    pub async fn begin_probe(
        &self,
        binary: impl Into<PathBuf>,
        idle_window: Option<Duration>,
    ) -> Result<u32, SessionError> {
        self.begin_probe_internal(binary.into(), idle_window, None)
            .await
    }

    /// Broker-aware form carrying cancellation, environment and timeout.
    pub async fn begin_probe_with_request(
        &self,
        request: ProcessLaunchRequest,
        idle_window: Option<Duration>,
    ) -> Result<u32, SessionError> {
        self.begin_probe_internal(request.binary.clone(), idle_window, Some(request))
            .await
    }

    async fn begin_probe_internal(
        &self,
        binary: PathBuf,
        idle_window: Option<Duration>,
        request: Option<ProcessLaunchRequest>,
    ) -> Result<u32, SessionError> {
        let mut reservation = {
            let mut state = self.inner.state.lock().await;
            state.active_probes += 1;
            if let Some(window) = idle_window.filter(|window| !window.is_zero()) {
                state.idle_window = state.idle_window.max(window);
            }
            if let Some(task) = state.idle_task.take() {
                task.abort();
            }
            ProbeReservation::new(self)
        };
        let _lifecycle = self.inner.lifecycle.lock().await;
        let result = self.ensure_started_locked(&binary, request).await;
        if result.is_err() {
            reservation.release().await;
        } else {
            let mut state = self.inner.state.lock().await;
            state.active_session_probes += 1;
            reservation.disarm();
        }
        result
    }

    async fn release_probe_reservation(&self) {
        let reset_requested = {
            let mut state = self.inner.state.lock().await;
            state.active_probes = state.active_probes.saturating_sub(1);
            state.active_probes == 0 && state.reset_requested
        };
        self.inner.changed.notify_waiters();
        if reset_requested {
            let session = self.clone();
            tokio::spawn(async move {
                session.reset().await;
            });
        } else {
            self.arm_idle_timer().await;
        }
    }

    async fn ensure_started_locked(
        &self,
        binary: &Path,
        request: Option<ProcessLaunchRequest>,
    ) -> Result<u32, SessionError> {
        self.reap_recorded_sessions().await;
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let existing = {
                let state = self.inner.state.lock().await;
                let threshold = self.inner.dependencies.failure_relaunch_threshold.max(1);
                state.process.clone().filter(|process| {
                    process.is_running()
                        && state.binary.as_deref() == Some(binary)
                        && !state.reset_requested
                        && !state.hard_reset_requested
                        && state.failures < threshold
                })
            };
            if let Some(process) = existing {
                return Ok(process.pid());
            }

            let wait_for_probe = {
                let state = self.inner.state.lock().await;
                state.process.is_some() && state.active_session_probes > 0
            };
            if wait_for_probe {
                changed.await;
                continue;
            }

            let replacement = {
                let state = self.inner.state.lock().await;
                state.process.as_ref().map(|_| {
                    let force = state.hard_reset_requested || state.failures > 0;
                    let cause = state.pending_cause.unwrap_or(
                        if state.failures
                            >= self.inner.dependencies.failure_relaunch_threshold.max(1)
                        {
                            ResetCause::Unhealthy
                        } else {
                            ResetCause::Deferred
                        },
                    );
                    (force, cause)
                })
            };
            if let Some((force, cause)) = replacement {
                self.stop_current(force, cause).await;
            }

            let process = match request.clone() {
                Some(request) => self.inner.dependencies.launcher.launch_with(request).await,
                None => self.inner.dependencies.launcher.launch(binary).await,
            }
            .map_err(SessionError::LaunchFailed)?;
            let pid = process.pid();
            let mut state = self.inner.state.lock().await;
            state.process = Some(process);
            state.binary = Some(binary.to_path_buf());
            state.reset_requested = false;
            state.hard_reset_requested = false;
            state.pending_cause = None;
            state.failures = 0;
            state.recent_output.clear();
            state.authentication_prompt_observed = false;
            state.generation = state.generation.wrapping_add(1);
            let process_for_record = state.process.clone();
            drop(state);
            if let Some(process) = process_for_record {
                self.persist_record(&process, binary).await;
            }
            return Ok(pid);
        }
    }

    async fn reap_recorded_sessions(&self) {
        let (Some(store), Some(identity), Some(lock)) = (
            self.inner.dependencies.record_store.clone(),
            self.inner.dependencies.identity_provider.clone(),
            self.inner.dependencies.launch_lock.clone(),
        ) else {
            return;
        };
        if lock.acquire().await.is_err() {
            return;
        }
        let _held_lock = HeldLaunchLock(lock);
        let records = store.load().unwrap_or_default();
        let owned_pid = self.pid().await;
        for record in records {
            if owned_pid == Some(record.pid) {
                continue;
            }
            let Some(live) = identity.identity(record.pid) else {
                let _ = store.remove(&record);
                continue;
            };
            if live.executable_path != record.executable_path
                || (live.start_epoch - record.start_epoch).abs() >= 0.001
            {
                let _ = store.remove(&record);
                continue;
            }
            let protected = match (
                record.owner_pid,
                record.owner_executable_path.as_deref(),
                record.owner_start_epoch,
            ) {
                (Some(owner_pid), Some(owner_path), Some(owner_start))
                    if owner_pid != identity.current_pid() =>
                {
                    identity.identity(owner_pid).is_some_and(|owner| {
                        owner.executable_path == owner_path
                            && (owner.start_epoch - owner_start).abs() < 0.001
                    })
                }
                _ => false,
            };
            if protected {
                continue;
            }
            identity.terminate_record(record.clone()).await;
            let _ = store.remove(&record);
        }
    }

    async fn persist_record(&self, process: &Arc<dyn ProcessHandle>, binary: &Path) {
        let (Some(store), Some(identity), Some(lock)) = (
            self.inner.dependencies.record_store.clone(),
            self.inner.dependencies.identity_provider.clone(),
            self.inner.dependencies.launch_lock.clone(),
        ) else {
            return;
        };
        let Some(process_identity) = identity
            .identity(process.pid())
            .or_else(|| process.identity())
        else {
            return;
        };
        let owner = identity.current_identity();
        let record = SessionRecord {
            pid: process.pid(),
            requested_binary_path: binary.to_path_buf(),
            executable_path: process_identity.executable_path,
            start_epoch: process_identity.start_epoch,
            process_group: process.process_group(),
            owner_pid: Some(identity.current_pid()),
            owner_executable_path: owner.as_ref().map(|value| value.executable_path.clone()),
            owner_start_epoch: owner.map(|value| value.start_epoch),
            marker: process.marker(),
        };
        if lock.acquire().await.is_ok() {
            let _held_lock = HeldLaunchLock(lock);
            let _ = store.save(&record);
            self.inner.state.lock().await.record = Some(record);
        }
    }

    pub async fn finish_probe(
        &self,
        success: bool,
        reset_after_fetch: bool,
        force_terminate: bool,
    ) {
        let (should_reset, force, cause) = {
            let mut state = self.inner.state.lock().await;
            if success {
                state.failures = 0;
            } else {
                state.failures = state.failures.saturating_add(1);
            }
            state.active_probes = state.active_probes.saturating_sub(1);
            state.active_session_probes = state.active_session_probes.saturating_sub(1);
            let unhealthy = !success
                && state.failures >= self.inner.dependencies.failure_relaunch_threshold.max(1);
            let should_reset =
                force_terminate || reset_after_fetch || state.reset_requested || unhealthy;
            let current = ResetCause::select(force_terminate, reset_after_fetch, unhealthy);
            if should_reset && state.active_probes > 0 {
                state.reset_requested = true;
                state.pending_cause =
                    Some(state.pending_cause.map_or(current, |old| old.max(current)));
                if !success || force_terminate {
                    state.hard_reset_requested = true;
                }
                self.inner.changed.notify_waiters();
                (false, false, current)
            } else if should_reset {
                let force = !success || force_terminate || state.hard_reset_requested;
                let cause = state.pending_cause.map_or(current, |old| old.max(current));
                state.reset_requested = false;
                state.hard_reset_requested = false;
                state.pending_cause = None;
                (true, force, cause)
            } else {
                (false, false, current)
            }
        };

        self.inner.changed.notify_waiters();
        if should_reset {
            let _lifecycle = self.inner.lifecycle.lock().await;
            self.stop_current(force, cause).await;
        } else {
            self.arm_idle_timer().await;
        }
    }

    async fn arm_idle_timer(&self) {
        let mut state = self.inner.state.lock().await;
        let Some(_) = state.process else { return };
        if state.active_probes != 0 || state.idle_window.is_zero() {
            return;
        }
        if let Some(task) = state.idle_task.take() {
            task.abort();
        }
        let generation = state.generation;
        let session = self.clone();
        let duration = state.idle_window;
        let sleep = session.inner.dependencies.clock.sleep(duration);
        state.idle_task = Some(tokio::spawn(async move {
            sleep.await;
            let _lifecycle = session.inner.lifecycle.lock().await;
            let matches = {
                let state = session.inner.state.lock().await;
                state.generation == generation && state.active_probes == 0
            };
            if matches {
                let force = {
                    let state = session.inner.state.lock().await;
                    state.hard_reset_requested || state.failures > 0
                };
                session.stop_current(force, ResetCause::Deferred).await;
            }
        }));
    }

    pub async fn reset(&self) {
        {
            let mut state = self.inner.state.lock().await;
            if let Some(task) = state.idle_task.take() {
                task.abort();
            }
            if state.active_probes != 0 {
                state.reset_requested = true;
                self.inner.changed.notify_waiters();
                return;
            }
            state.reset_requested = false;
            state.hard_reset_requested |= state.failures != 0;
        }
        let _lifecycle = self.inner.lifecycle.lock().await;
        let force = {
            let state = self.inner.state.lock().await;
            state.hard_reset_requested || state.failures != 0
        };
        let cause = self
            .inner
            .state
            .lock()
            .await
            .pending_cause
            .take()
            .unwrap_or(ResetCause::Deferred);
        self.stop_current(force, cause).await;
        self.reap_recorded_sessions().await;
    }

    async fn stop_current(&self, force: bool, cause: ResetCause) {
        let process = {
            let mut state = self.inner.state.lock().await;
            state.last_stop_reason = Some(cause.message());
            state.binary = None;
            state.generation = state.generation.wrapping_add(1);
            state.idle_window = self.inner.dependencies.idle_window;
            let record = state.record.take();
            state.recent_output.clear();
            state.authentication_prompt_observed = false;
            (state.process.take(), record)
        };
        if let Some(process) = process.0 {
            if force {
                process.terminate().await;
            } else {
                process.close_gracefully().await;
            }
        }
        if let Some(record) = process.1 {
            self.remove_record(&record).await;
        }
        self.inner.changed.notify_waiters();
    }

    async fn remove_record(&self, record: &SessionRecord) {
        let (Some(store), Some(lock)) = (
            self.inner.dependencies.record_store.clone(),
            self.inner.dependencies.launch_lock.clone(),
        ) else {
            return;
        };
        if lock.acquire().await.is_ok() {
            let _held_lock = HeldLaunchLock(lock);
            let _ = store.remove(record);
        }
    }

    /// Returns all newly available output and retains only a 4096-byte tail.
    pub async fn drain_output(&self) -> Vec<u8> {
        let mut state = self.inner.state.lock().await;
        let mut output = state.recent_output.clone();
        if let Some(process) = state.process.clone() {
            if let Ok(drained) = process.drain_output().await {
                output.extend(drained);
            }
            state.recent_output = output.iter().copied().rev().take(4096).collect::<Vec<_>>();
            state.recent_output.reverse();
        }
        if !state.authentication_prompt_observed && AuthenticationPrompt::contains(&output) {
            state.authentication_prompt_observed = true;
        }
        if state.authentication_prompt_observed && !AuthenticationPrompt::contains(&output) {
            output.extend_from_slice(AuthenticationPrompt::EVIDENCE);
        }
        output
    }

    #[must_use]
    pub async fn pid(&self) -> Option<u32> {
        let state = self.inner.state.lock().await;
        state
            .process
            .as_ref()
            .filter(|process| process.is_running())
            .map(|process| process.pid())
    }

    #[must_use]
    pub async fn is_running(&self) -> bool {
        self.pid().await.is_some()
    }

    #[must_use]
    pub async fn failure_count(&self) -> usize {
        self.inner.state.lock().await.failures
    }

    #[must_use]
    pub async fn idle_window(&self) -> Duration {
        self.inner.state.lock().await.idle_window
    }

    #[must_use]
    pub async fn last_stop_reason(&self) -> Option<&'static str> {
        self.inner.state.lock().await.last_stop_reason
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthenticationPrompt, BrokerProcessLauncher, Dependencies};
    use crate::brokers::process::ProcessRegistry;
    use ariadusage_core::gates::launch::LaunchGate;
    use std::sync::Arc;

    fn dependencies() -> Dependencies {
        Dependencies::new(BrokerProcessLauncher::new(
            Arc::new(ProcessRegistry::new()),
            Arc::new(LaunchGate::default()),
        ))
    }

    #[test]
    fn unavailable_state_directory_disables_session_coordination() {
        let dependencies =
            dependencies().with_state_directory(Err(crate::paths::PathError::NoHomeDirectory));
        assert!(dependencies.record_store.is_none());
        assert!(dependencies.identity_provider.is_none());
        assert!(dependencies.launch_lock.is_none());
    }

    #[test]
    fn session_records_use_the_resolved_state_directory() {
        let home = tempfile::tempdir().unwrap();
        let state_directory =
            crate::paths::resolve_state_dir(|_| Some(" relative-state ".into()), Some(home.path()))
                .unwrap();
        let dependencies = dependencies().with_state_directory(Ok(state_directory));
        let record = super::SessionRecord {
            pid: 123,
            requested_binary_path: "/synthetic/agy".into(),
            executable_path: "/synthetic/agy".into(),
            start_epoch: 100.0,
            process_group: Some(123),
            owner_pid: None,
            owner_executable_path: None,
            owner_start_epoch: None,
            marker: Some("synthetic-marker".into()),
        };
        dependencies.record_store.unwrap().save(&record).unwrap();
        let path = home
            .path()
            .join(".local/state/antigravity/agy-session.json");
        let records: Vec<super::SessionRecord> =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(records, vec![record]);
    }

    #[test]
    fn prompt_matching_preserves_auto_refresh_exception() {
        assert!(AuthenticationPrompt::contains(b"select  LOGIN\nmethod :"));
        assert!(AuthenticationPrompt::contains(
            b"You are not logged into Antigravity."
        ));
        assert!(AuthenticationPrompt::contains(
            b"KEYRING auth : timed out after 10s"
        ));
        assert!(AuthenticationPrompt::contains(
            b"keyringAuth: timed out after 10s"
        ));
        assert!(!AuthenticationPrompt::contains(
            b"Welcome. You are currently not signed in.\nSigning in..."
        ));
    }
}
