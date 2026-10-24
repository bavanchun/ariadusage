// Ported from CodexBar Tests/CodexBarTests/AntigravityCLISessionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

#[cfg(unix)]
#[path = "support/process.rs"]
mod process_support;

#[cfg(unix)]
use ariadusage_core::gates::launch::LaunchGate;
#[cfg(unix)]
use ariadusage_core::pipeline::FetchInteraction;
#[cfg(unix)]
use ariadusage_engine::brokers::process::{ProcessEnv, ProcessRegistry, PtySession, PtySize};
use ariadusage_engine::providers::antigravity::{
    AntigravityCLISession, AuthenticationPrompt, Dependencies, FileSessionLaunchLock,
    FileSessionRecordStore, ProcessFuture, ProcessHandle, ProcessLauncher, ResetCause,
    SessionClock, SessionIdentityProvider, SessionLaunchLock, SessionProcessIdentity,
    SessionRecord, SessionRecordStore,
};

#[derive(Default)]
struct FakeProcess {
    pid: u32,
    running: Mutex<bool>,
    output: Mutex<VecDeque<Vec<u8>>>,
    events: Mutex<Vec<&'static str>>,
    identity: Mutex<Option<SessionProcessIdentity>>,
    process_group: Option<u32>,
    marker: Option<String>,
    close_started: Mutex<Option<Arc<Notify>>>,
    close_release: Mutex<Option<Arc<Notify>>>,
}

impl FakeProcess {
    fn new(pid: u32) -> Arc<Self> {
        Arc::new(Self {
            pid,
            running: Mutex::new(true),
            output: Mutex::new(VecDeque::new()),
            events: Mutex::new(Vec::new()),
            identity: Mutex::new(None),
            process_group: None,
            marker: None,
            close_started: Mutex::new(None),
            close_release: Mutex::new(None),
        })
    }

    fn events(&self) -> Vec<&'static str> {
        self.events.lock().unwrap().clone()
    }

    fn stop(&self) {
        *self.running.lock().unwrap() = false;
    }
}

impl ProcessHandle for FakeProcess {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn is_running(&self) -> bool {
        *self.running.lock().unwrap()
    }

    fn identity(&self) -> Option<SessionProcessIdentity> {
        self.identity.lock().unwrap().clone()
    }

    fn process_group(&self) -> Option<u32> {
        self.process_group
    }

    fn marker(&self) -> Option<String> {
        self.marker.clone()
    }

    fn drain_output(&self) -> ProcessFuture<Result<Vec<u8>, String>> {
        let output = self.output.lock().unwrap().pop_front().unwrap_or_default();
        Box::pin(async move { Ok(output) })
    }

    fn close_gracefully(&self) -> ProcessFuture<()> {
        self.events.lock().unwrap().push("graceful");
        *self.running.lock().unwrap() = false;
        let started = self.close_started.lock().unwrap().clone();
        let release = self.close_release.lock().unwrap().clone();
        Box::pin(async move {
            if let (Some(started), Some(release)) = (started, release) {
                started.notify_one();
                release.notified().await;
            }
        })
    }

    fn terminate(&self) -> ProcessFuture<()> {
        self.events.lock().unwrap().push("force");
        *self.running.lock().unwrap() = false;
        Box::pin(async {})
    }
}

#[derive(Default)]
struct MemoryStore {
    records: Mutex<Vec<SessionRecord>>,
    fail_save: bool,
}

impl MemoryStore {
    fn with(records: impl IntoIterator<Item = SessionRecord>) -> Arc<Self> {
        Arc::new(Self {
            records: Mutex::new(records.into_iter().collect()),
            fail_save: false,
        })
    }

    fn snapshot(&self) -> Vec<SessionRecord> {
        self.records.lock().unwrap().clone()
    }
}

impl SessionRecordStore for MemoryStore {
    fn load(&self) -> Result<Vec<SessionRecord>, String> {
        Ok(self.snapshot())
    }

    fn save(&self, record: &SessionRecord) -> Result<(), String> {
        if self.fail_save {
            return Err("save rejected".to_owned());
        }
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }

    fn remove(&self, record: &SessionRecord) -> Result<(), String> {
        self.records.lock().unwrap().retain(|item| item != record);
        Ok(())
    }
}

#[derive(Default)]
struct FakeIdentity {
    identities: Mutex<BTreeMap<u32, SessionProcessIdentity>>,
    current_pid: u32,
    current: Mutex<Option<SessionProcessIdentity>>,
    terminations: Mutex<Vec<SessionRecord>>,
}

impl FakeIdentity {
    fn current(pid: u32, executable_path: &str, start_epoch: f64) -> Arc<Self> {
        Arc::new(Self {
            identities: Mutex::new(BTreeMap::new()),
            current_pid: pid,
            current: Mutex::new(Some(SessionProcessIdentity {
                executable_path: executable_path.to_owned(),
                start_epoch,
            })),
            terminations: Mutex::new(Vec::new()),
        })
    }

    fn set(&self, pid: u32, executable_path: &str, start_epoch: f64) {
        self.identities.lock().unwrap().insert(
            pid,
            SessionProcessIdentity {
                executable_path: executable_path.to_owned(),
                start_epoch,
            },
        );
    }
}

impl SessionIdentityProvider for FakeIdentity {
    fn identity(&self, pid: u32) -> Option<SessionProcessIdentity> {
        self.identities.lock().unwrap().get(&pid).cloned()
    }

    fn current_pid(&self) -> u32 {
        self.current_pid
    }

    fn current_identity(&self) -> Option<SessionProcessIdentity> {
        self.current.lock().unwrap().clone()
    }

    fn terminate_record(&self, record: SessionRecord) -> ProcessFuture<()> {
        self.terminations.lock().unwrap().push(record);
        Box::pin(async {})
    }
}

#[derive(Default)]
struct FakeLock {
    held: Mutex<bool>,
}

impl SessionLaunchLock for FakeLock {
    fn acquire(&self) -> ProcessFuture<Result<(), String>> {
        let mut held = self.held.lock().unwrap();
        if *held {
            return Box::pin(async { Err("lock held".to_owned()) });
        }
        *held = true;
        Box::pin(async { Ok(()) })
    }

    fn release(&self) {
        *self.held.lock().unwrap() = false;
    }
}

struct FakeLauncher {
    next: Mutex<u32>,
    launched: Mutex<Vec<PathBuf>>,
    processes: Mutex<Vec<Arc<FakeProcess>>>,
}

impl FakeLauncher {
    fn new() -> Arc<Self> {
        Self::with_next_pid(10)
    }

    fn with_next_pid(pid: u32) -> Arc<Self> {
        Arc::new(Self {
            next: Mutex::new(pid),
            launched: Mutex::new(Vec::new()),
            processes: Mutex::new(Vec::new()),
        })
    }
}

#[derive(Default)]
struct ManualClock {
    sleepers: Mutex<VecDeque<Arc<Notify>>>,
    changed: Notify,
}

impl ManualClock {
    async fn wait_for_sleeps(&self, expected: usize) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.sleepers.lock().unwrap().len() >= expected {
                return;
            }
            notified.await;
        }
    }

    fn resume_all(&self) {
        for sleeper in self.sleepers.lock().unwrap().drain(..) {
            sleeper.notify_one();
        }
    }
}

impl SessionClock for ManualClock {
    fn sleep(&self, _duration: Duration) -> ProcessFuture<()> {
        let sleeper = Arc::new(Notify::new());
        self.sleepers
            .lock()
            .unwrap()
            .push_back(Arc::clone(&sleeper));
        self.changed.notify_waiters();
        Box::pin(async move { sleeper.notified().await })
    }
}

async fn wait_until_stopped(session: &AntigravityCLISession) {
    for _ in 0..100 {
        if !session.is_running().await {
            return;
        }
        tokio::task::yield_now().await;
    }
    assert!(!session.is_running().await, "session did not stop");
}

impl ProcessLauncher for FakeLauncher {
    fn launch(&self, binary: &Path) -> ProcessFuture<Result<Arc<dyn ProcessHandle>, String>> {
        let pid = {
            let mut next = self.next.lock().unwrap();
            let pid = *next;
            *next += 1;
            pid
        };
        self.launched.lock().unwrap().push(binary.to_path_buf());
        let process = FakeProcess::new(pid);
        *process.identity.lock().unwrap() = Some(SessionProcessIdentity {
            executable_path: binary.to_string_lossy().into_owned(),
            start_epoch: pid as f64,
        });
        self.processes.lock().unwrap().push(Arc::clone(&process));
        Box::pin(async move { Ok(process as Arc<dyn ProcessHandle>) })
    }
}

struct BlockingLauncher {
    started: Arc<Notify>,
    release: Arc<Notify>,
    process: Arc<FakeProcess>,
}

impl BlockingLauncher {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            process: FakeProcess::new(77),
        })
    }
}

impl ProcessLauncher for BlockingLauncher {
    fn launch(&self, _binary: &Path) -> ProcessFuture<Result<Arc<dyn ProcessHandle>, String>> {
        let started = Arc::clone(&self.started);
        let release = Arc::clone(&self.release);
        let process = Arc::clone(&self.process);
        Box::pin(async move {
            started.notify_one();
            release.notified().await;
            Ok(process as Arc<dyn ProcessHandle>)
        })
    }
}

// Companion cancellation assertion for the probe reservation lifecycle.
#[tokio::test]
async fn cancelled_begin_probe_releases_its_probe_reservation() {
    let launcher = BlockingLauncher::new();
    let session = AntigravityCLISession::new(Dependencies::new(
        Arc::clone(&launcher) as Arc<dyn ProcessLauncher>
    ));
    let pending = {
        let session = session.clone();
        tokio::spawn(async move { session.begin_probe("/bin/agy", None).await })
    };
    launcher.started.notified().await;
    pending.abort();
    let _ = pending.await;
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    launcher.release.notify_one();
    assert_eq!(session.begin_probe("/bin/agy", None).await.unwrap(), 77);
    session.finish_probe(true, true, false).await;
    assert!(!session.is_running().await);
}

struct KernelIdentityLauncher;

impl ProcessLauncher for KernelIdentityLauncher {
    fn launch(&self, _binary: &Path) -> ProcessFuture<Result<Arc<dyn ProcessHandle>, String>> {
        let process = FakeProcess::new(91);
        *process.identity.lock().unwrap() = Some(SessionProcessIdentity {
            executable_path: "/usr/bin/agy-real".to_owned(),
            start_epoch: 91.0,
        });
        Box::pin(async move { Ok(process as Arc<dyn ProcessHandle>) })
    }
}

// Companion persistence assertion for a requested symlink resolving to a kernel executable.
#[tokio::test]
async fn persistence_keeps_requested_path_separate_from_kernel_identity() {
    let store = MemoryStore::with(std::iter::empty::<SessionRecord>());
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    let dependencies =
        Dependencies::new(Arc::new(KernelIdentityLauncher) as Arc<dyn ProcessLauncher>)
            .with_session_coordination(
                Arc::clone(&store) as Arc<dyn SessionRecordStore>,
                identity,
                Arc::new(FakeLock::default()),
            );
    let session = AntigravityCLISession::new(dependencies);
    session.begin_probe("/tmp/agy-link", None).await.unwrap();
    let records = store.snapshot();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].requested_binary_path,
        PathBuf::from("/tmp/agy-link")
    );
    assert_eq!(records[0].executable_path, "/usr/bin/agy-real");
    session.finish_probe(true, true, false).await;
}

fn session(launcher: Arc<FakeLauncher>) -> AntigravityCLISession {
    let mut dependencies = Dependencies::new(launcher);
    dependencies.idle_window = Duration::from_secs(60);
    dependencies.failure_relaunch_threshold = 2;
    AntigravityCLISession::new(dependencies)
}

fn coordinated_session(
    launcher: Arc<FakeLauncher>,
    store: Arc<MemoryStore>,
    identity: Arc<FakeIdentity>,
) -> AntigravityCLISession {
    let mut dependencies = Dependencies::new(launcher);
    dependencies.idle_window = Duration::from_secs(60);
    dependencies.failure_relaunch_threshold = 2;
    dependencies.record_store = Some(store);
    dependencies.identity_provider = Some(identity);
    dependencies.launch_lock = Some(Arc::new(FakeLock::default()));
    AntigravityCLISession::new(dependencies)
}

fn record(pid: u32, binary: &str, start_epoch: f64, owner_pid: Option<u32>) -> SessionRecord {
    SessionRecord {
        pid,
        requested_binary_path: PathBuf::from(binary),
        executable_path: binary.to_owned(),
        start_epoch,
        process_group: Some(pid),
        owner_pid,
        owner_executable_path: owner_pid.map(|_| "/app/ariadusage".to_owned()),
        owner_start_epoch: owner_pid.map(|_| 1.0),
        marker: None,
    }
}

// upstream: AntigravityCLISessionTests.swift:438 reuses alive process for same binary
#[tokio::test]
async fn reuses_alive_process_for_same_binary() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    assert_eq!(session.begin_probe("/bin/agy", None).await.unwrap(), 10);
    session.finish_probe(true, false, false).await;
    assert_eq!(session.begin_probe("/bin/agy", None).await.unwrap(), 10);
    session.finish_probe(true, false, false).await;
    assert_eq!(
        launcher.launched.lock().unwrap().as_slice(),
        [PathBuf::from("/bin/agy")]
    );
    session.reset().await;
}

// upstream: AntigravityCLISessionTests.swift:455 relaunches when binary changes
#[tokio::test]
async fn replacing_binary_closes_previous_process() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/old/agy", None).await.unwrap();
    session.finish_probe(true, false, false).await;
    assert_eq!(session.begin_probe("/new/agy", None).await.unwrap(), 11);
    session.finish_probe(true, false, false).await;
    assert_eq!(
        launcher.launched.lock().unwrap().as_slice(),
        [PathBuf::from("/old/agy"), PathBuf::from("/new/agy")]
    );
    assert_eq!(
        launcher.processes.lock().unwrap()[0].events(),
        vec!["graceful"]
    );
    session.reset().await;
}

// upstream: AntigravityCLISessionTests.swift:1270 authentication reset never writes interactive exit input
#[tokio::test]
async fn failed_probe_forces_teardown_without_interactive_exit() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(false, true, true).await;
    assert_eq!(
        launcher.processes.lock().unwrap()[0].events(),
        vec!["force"]
    );
    assert!(!session.is_running().await);
}

// upstream: AntigravityCLISessionTests.swift:1333 repeated probe failures relaunch session
#[tokio::test]
async fn repeated_failures_relaunch_and_success_resets_counter() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    for _ in 0..2 {
        session.begin_probe("/bin/agy", None).await.unwrap();
        session.finish_probe(false, false, false).await;
    }
    assert_eq!(session.begin_probe("/bin/agy", None).await.unwrap(), 11);
    session.finish_probe(true, false, false).await;
    assert_eq!(
        launcher.launched.lock().unwrap().as_slice(),
        [PathBuf::from("/bin/agy"), PathBuf::from("/bin/agy")]
    );
    assert_eq!(session.failure_count().await, 0);
    session.reset().await;
}

// upstream: AntigravityCLISessionTests.swift:751 session keeps one rolling PTY buffer across concurrent probes
#[tokio::test]
async fn rolling_output_retains_authentication_evidence() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.begin_probe("/bin/agy", None).await.unwrap();
    launcher.processes.lock().unwrap()[0]
        .output
        .lock()
        .unwrap()
        .push_back(vec![0xE2, 0x96]);
    let first = session.drain_output().await;
    launcher.processes.lock().unwrap()[0]
        .output
        .lock()
        .unwrap()
        .push_back([vec![0x84], b"Select login method:".to_vec()].concat());
    let second = session.drain_output().await;
    let third = session.drain_output().await;
    assert_eq!(first, vec![0xE2, 0x96]);
    assert!(AuthenticationPrompt::contains(&second));
    assert_eq!(third, second);
    session.finish_probe(false, true, true).await;
    session.finish_probe(false, false, false).await;
}

// upstream: AntigravityCLISessionTests.swift:1351 session reset reasons distinguish authentication from unhealthy probes
#[test]
fn reset_cause_precedence_matches_upstream() {
    assert_eq!(
        ResetCause::select(true, true, true).message(),
        "authentication required"
    );
    assert_eq!(
        ResetCause::select(false, true, true).message(),
        "unhealthy CLI HTTPS session"
    );
    assert_eq!(
        ResetCause::select(false, true, false).message(),
        "one-shot CLI fetch"
    );
    assert_eq!(
        ResetCause::select(false, false, false).message(),
        "deferred reset"
    );
}

// upstream: AntigravityCLISessionTests.swift:1101 file store migrates legacy record and preserves independent owners
#[test]
fn file_store_migrates_legacy_record_and_preserves_independent_owners() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agy-session.json");
    let first = SessionRecord {
        pid: 10,
        requested_binary_path: PathBuf::from("/bin/agy"),
        executable_path: "/bin/agy".to_owned(),
        start_epoch: 100.0,
        process_group: Some(10),
        owner_pid: Some(900),
        owner_executable_path: Some("/app/ariadusage".to_owned()),
        owner_start_epoch: Some(1.0),
        marker: None,
    };
    std::fs::write(&path, serde_json::to_vec(&first).unwrap()).unwrap();
    let store = FileSessionRecordStore::new(path.clone());
    assert_eq!(store.load().unwrap(), vec![first.clone()]);

    let second = SessionRecord {
        pid: 20,
        requested_binary_path: PathBuf::from("/bin/agy"),
        executable_path: "/bin/agy".to_owned(),
        start_epoch: 200.0,
        process_group: Some(20),
        owner_pid: Some(901),
        owner_executable_path: Some("/app/ariadusage".to_owned()),
        owner_start_epoch: Some(2.0),
        marker: None,
    };
    store.save(&second).unwrap();
    let mut pids = store
        .load()
        .unwrap()
        .into_iter()
        .map(|record| record.pid)
        .collect::<Vec<_>>();
    pids.sort_unstable();
    assert_eq!(pids, vec![10, 20]);
    store.remove(&first).unwrap();
    assert_eq!(store.load().unwrap(), vec![second]);

    std::fs::write(&path, b"{").unwrap();
    store.save(&first).unwrap();
    assert_eq!(store.load().unwrap(), vec![first]);
}

// upstream: AntigravityCLISessionTests.swift:597 relaunches when existing process is dead
#[tokio::test]
async fn relaunches_when_existing_process_is_dead() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(true, false, false).await;
    launcher.processes.lock().unwrap()[0].stop();
    assert_eq!(session.begin_probe("/bin/agy", None).await.unwrap(), 11);
    session.finish_probe(true, false, false).await;
    assert_eq!(
        launcher.launched.lock().unwrap().as_slice(),
        [PathBuf::from("/bin/agy"), PathBuf::from("/bin/agy")]
    );
    session.reset().await;
}

// upstream: AntigravityCLISessionTests.swift:510 replacement waits for active probe before relaunching
#[tokio::test]
async fn replacement_waits_for_active_probe_before_relaunching() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/old/agy", None).await.unwrap();
    let replacement_started = Arc::new(Notify::new());
    let replacement = {
        let session = session.clone();
        let replacement_started = Arc::clone(&replacement_started);
        tokio::spawn(async move {
            replacement_started.notify_one();
            session.begin_probe("/new/agy", None).await.unwrap()
        })
    };
    replacement_started.notified().await;
    tokio::task::yield_now().await;
    assert!(!replacement.is_finished());
    assert_eq!(launcher.launched.lock().unwrap().len(), 1);
    session.finish_probe(true, false, false).await;
    assert_eq!(replacement.await.unwrap(), 11);
    session.finish_probe(true, false, false).await;
    assert_eq!(launcher.launched.lock().unwrap().len(), 2);
    session.reset().await;
}

// upstream: AntigravityCLISessionTests.swift:536 queued replacement hard stops a signed out process
#[tokio::test]
async fn queued_replacement_hard_stops_signed_out_process() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/old/agy", None).await.unwrap();
    let replacement_started = Arc::new(Notify::new());
    let replacement = {
        let session = session.clone();
        let replacement_started = Arc::clone(&replacement_started);
        tokio::spawn(async move {
            replacement_started.notify_one();
            session.begin_probe("/new/agy", None).await.unwrap()
        })
    };
    replacement_started.notified().await;
    tokio::task::yield_now().await;
    assert!(!replacement.is_finished());
    assert_eq!(launcher.launched.lock().unwrap().len(), 1);
    session.finish_probe(false, true, true).await;
    assert_eq!(replacement.await.unwrap(), 11);
    assert_eq!(
        launcher.processes.lock().unwrap()[0].events(),
        vec!["force"]
    );
    assert_eq!(
        session.last_stop_reason().await,
        Some("authentication required")
    );
    session.finish_probe(true, true, false).await;
}

// upstream: AntigravityCLISessionTests.swift:562 replacement ignores queued starters while waiting for active probe
#[tokio::test]
async fn queued_replacements_share_one_new_process() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/old/agy", None).await.unwrap();
    let first_started = Arc::new(Notify::new());
    let second_started = Arc::new(Notify::new());
    let first = {
        let session = session.clone();
        let first_started = Arc::clone(&first_started);
        tokio::spawn(async move {
            first_started.notify_one();
            session.begin_probe("/new/agy", None).await.unwrap()
        })
    };
    let second = {
        let session = session.clone();
        let second_started = Arc::clone(&second_started);
        tokio::spawn(async move {
            second_started.notify_one();
            session.begin_probe("/new/agy", None).await.unwrap()
        })
    };
    first_started.notified().await;
    second_started.notified().await;
    tokio::task::yield_now().await;
    assert!(!first.is_finished());
    assert!(!second.is_finished());
    assert_eq!(launcher.launched.lock().unwrap().len(), 1);
    session.finish_probe(true, false, false).await;
    assert_eq!(first.await.unwrap(), 11);
    assert_eq!(second.await.unwrap(), 11);
    assert_eq!(launcher.launched.lock().unwrap().len(), 2);
    session.finish_probe(true, false, false).await;
    session.finish_probe(true, false, false).await;
    session.reset().await;
}

// upstream: AntigravityCLISessionTests.swift:1299 concurrent success preserves a failed probes deferred hard reset
#[tokio::test]
async fn concurrent_success_preserves_deferred_hard_reset() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(false, true, false).await;
    assert!(session.is_running().await);
    session.finish_probe(true, false, false).await;
    assert!(!session.is_running().await);
    assert_eq!(
        launcher.processes.lock().unwrap()[0].events(),
        vec!["force"]
    );
}

// upstream: AntigravityCLISessionTests.swift:1241 one shot CLI reset tears down after fetch
#[tokio::test]
async fn one_shot_reset_tears_down_after_fetch() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(true, true, false).await;
    assert!(!session.is_running().await);
    assert_eq!(
        launcher.processes.lock().unwrap()[0].events(),
        vec!["graceful"]
    );
}

// upstream: AntigravityCLISessionTests.swift:1253 one shot CLI reset is deferred until all active probes finish
#[tokio::test]
async fn one_shot_reset_is_deferred_until_all_probes_finish() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(true, true, false).await;
    assert!(session.is_running().await);
    session.finish_probe(true, false, false).await;
    assert!(!session.is_running().await);
}

// Companion assertion for the authentication teardown branch.
#[tokio::test]
async fn authentication_reset_uses_force_teardown() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(false, true, true).await;
    assert_eq!(
        launcher.processes.lock().unwrap()[0].events(),
        vec!["force"]
    );
}

// upstream: AntigravityCLISessionTests.swift:1285 failed reset never writes interactive exit input
#[tokio::test]
async fn failed_reset_uses_force_teardown() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(false, true, false).await;
    assert_eq!(
        launcher.processes.lock().unwrap()[0].events(),
        vec!["force"]
    );
}

// upstream: AntigravityCLISessionTests.swift:1397 success resets failure counter
#[tokio::test]
async fn success_resets_failure_counter() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(false, false, false).await;
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(true, false, false).await;
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(false, false, false).await;
    assert_eq!(
        launcher.launched.lock().unwrap().as_slice(),
        [PathBuf::from("/bin/agy")]
    );
    assert_eq!(session.failure_count().await, 1);
}

// Companion lifecycle assertion for the reset-cause contract covered below.
#[tokio::test]
async fn reset_reason_is_recorded() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(false, true, true).await;
    assert_eq!(
        session.last_stop_reason().await,
        Some("authentication required")
    );
}

// upstream: AntigravityCLISessionTests.swift:1371 deferred unhealthy reset preserves its cause after a concurrent success
#[tokio::test]
async fn deferred_unhealthy_reset_preserves_cause() {
    let launcher = FakeLauncher::new();
    let mut dependencies = Dependencies::new(Arc::clone(&launcher) as Arc<dyn ProcessLauncher>);
    dependencies.idle_window = Duration::from_secs(60);
    dependencies.failure_relaunch_threshold = 1;
    let session = AntigravityCLISession::new(dependencies);
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(false, false, false).await;
    session.finish_probe(true, false, false).await;
    assert_eq!(
        session.last_stop_reason().await,
        Some("unhealthy CLI HTTPS session")
    );
}

// upstream: AntigravityCLISessionTests.swift:1384 deferred authentication reset preserves its cause after a concurrent success
#[tokio::test]
async fn deferred_authentication_reset_preserves_cause() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(false, true, true).await;
    session.finish_probe(true, false, false).await;
    assert_eq!(
        session.last_stop_reason().await,
        Some("authentication required")
    );
}

// upstream: AntigravityCLISessionTests.swift:919 reset reaps persisted stale session when no in memory process exists
#[tokio::test]
async fn reset_reaps_persisted_stale_session() {
    let stale = record(777, "/bin/agy", 42.0, None);
    let store = MemoryStore::with([stale.clone()]);
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    identity.set(777, "/bin/agy", 42.0);
    let session = coordinated_session(
        FakeLauncher::new(),
        Arc::clone(&store),
        Arc::clone(&identity),
    );
    session.reset().await;
    assert!(store.snapshot().is_empty());
    assert_eq!(identity.terminations.lock().unwrap().as_slice(), &[stale]);
}

// upstream: AntigravityCLISessionTests.swift:938 reset preserves persisted session owned by another live process
#[tokio::test]
async fn reset_preserves_session_owned_by_live_process() {
    let protected = record(777, "/bin/agy", 42.0, Some(900));
    let store = MemoryStore::with([protected.clone()]);
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    identity.set(777, "/bin/agy", 42.0);
    identity.set(900, "/app/ariadusage", 1.0);
    let session = coordinated_session(
        FakeLauncher::new(),
        Arc::clone(&store),
        Arc::clone(&identity),
    );
    session.reset().await;
    assert_eq!(store.snapshot(), vec![protected]);
    assert!(identity.terminations.lock().unwrap().is_empty());
}

// upstream: AntigravityCLISessionTests.swift:962 launch tracks an independent session while another process is live
#[tokio::test]
async fn launch_tracks_independent_session_while_peer_is_live() {
    let protected = record(777, "/bin/agy", 42.0, Some(900));
    let store = MemoryStore::with([protected.clone()]);
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    identity.set(777, "/bin/agy", 42.0);
    identity.set(900, "/app/ariadusage", 1.0);
    let launcher = FakeLauncher::new();
    let session = coordinated_session(
        Arc::clone(&launcher),
        Arc::clone(&store),
        Arc::clone(&identity),
    );
    session.begin_probe("/bin/agy", None).await.unwrap();
    assert_eq!(launcher.launched.lock().unwrap().len(), 1);
    assert!(identity.terminations.lock().unwrap().is_empty());
    let mut pids = store
        .snapshot()
        .into_iter()
        .map(|record| record.pid)
        .collect::<Vec<_>>();
    pids.sort_unstable();
    assert_eq!(pids, vec![10, 777]);
    session.finish_probe(true, true, false).await;
    assert!(store.snapshot().contains(&protected));
}

// upstream: AntigravityCLISessionTests.swift:994 different binary tracks an independent session while another process is live
#[tokio::test]
async fn different_binary_tracks_independent_session() {
    let protected = record(777, "/bin/agy", 42.0, Some(900));
    let store = MemoryStore::with([protected.clone()]);
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    identity.set(777, "/bin/agy", 42.0);
    identity.set(900, "/app/ariadusage", 1.0);
    let launcher = FakeLauncher::new();
    let session = coordinated_session(
        Arc::clone(&launcher),
        Arc::clone(&store),
        Arc::clone(&identity),
    );
    session.begin_probe("/new/agy", None).await.unwrap();
    assert_eq!(
        launcher.launched.lock().unwrap().as_slice(),
        [PathBuf::from("/new/agy")]
    );
    assert!(identity.terminations.lock().unwrap().is_empty());
    let mut pids = store
        .snapshot()
        .into_iter()
        .map(|record| record.pid)
        .collect::<Vec<_>>();
    pids.sort_unstable();
    assert_eq!(pids, vec![10, 777]);
    session.finish_probe(true, true, false).await;
    assert!(store.snapshot().contains(&protected));
}

// upstream: AntigravityCLISessionTests.swift:1025 concurrent hosts atomically track independent sessions
#[tokio::test]
async fn concurrent_hosts_track_independent_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let record_path = directory.path().join("agy-session.json");
    let lock_path = directory.path().join("agy-session.lock");
    let first_store = Arc::new(FileSessionRecordStore::new(record_path.clone()));
    let second_store = Arc::new(FileSessionRecordStore::new(record_path));
    let first_identity = FakeIdentity::current(900, "/app/ariadusage", 1.0);
    let second_identity = FakeIdentity::current(901, "/app/ariadusage", 2.0);
    for identity in [&first_identity, &second_identity] {
        identity.set(10, "/bin/agy", 100.0);
        identity.set(20, "/bin/agy", 200.0);
        identity.set(900, "/app/ariadusage", 1.0);
        identity.set(901, "/app/ariadusage", 2.0);
    }
    let first_launcher = FakeLauncher::with_next_pid(10);
    let second_launcher = FakeLauncher::with_next_pid(20);
    let first = AntigravityCLISession::new(
        Dependencies::new(Arc::clone(&first_launcher) as Arc<dyn ProcessLauncher>)
            .with_session_coordination(
                first_store,
                first_identity,
                FileSessionLaunchLock::new(lock_path.clone()),
            ),
    );
    let second = AntigravityCLISession::new(
        Dependencies::new(Arc::clone(&second_launcher) as Arc<dyn ProcessLauncher>)
            .with_session_coordination(
                Arc::clone(&second_store) as Arc<dyn SessionRecordStore>,
                second_identity,
                FileSessionLaunchLock::new(lock_path),
            ),
    );
    let (left, right) = tokio::join!(
        first.begin_probe("/bin/agy", None),
        second.begin_probe("/bin/agy", None)
    );
    assert!(left.is_ok() && right.is_ok());
    first.finish_probe(true, false, false).await;
    second.finish_probe(true, false, false).await;
    assert_eq!(first_launcher.launched.lock().unwrap().len(), 1);
    assert_eq!(second_launcher.launched.lock().unwrap().len(), 1);
    let mut pids = second_store
        .load()
        .unwrap()
        .into_iter()
        .map(|record| record.pid)
        .collect::<Vec<_>>();
    pids.sort_unstable();
    assert_eq!(pids, vec![10, 20]);
    first.reset().await;
    second.reset().await;
    assert!(second_store.load().unwrap().is_empty());
}

// upstream: AntigravityCLISessionTests.swift:1064 warm reuse reaps a crashed peer session
#[tokio::test]
async fn warm_reuse_reaps_crashed_peer_session() {
    let store = MemoryStore::with(std::iter::empty::<SessionRecord>());
    let identity = FakeIdentity::current(900, "/app/ariadusage", 1.0);
    identity.set(900, "/app/ariadusage", 1.0);
    identity.set(901, "/app/ariadusage", 1.0);
    identity.set(20, "/bin/agy", 20.0);
    let launcher = FakeLauncher::new();
    let session = coordinated_session(
        Arc::clone(&launcher),
        Arc::clone(&store),
        Arc::clone(&identity),
    );
    assert_eq!(session.begin_probe("/bin/agy", None).await.unwrap(), 10);
    session.finish_probe(true, false, false).await;
    let peer = record(20, "/bin/agy", 20.0, Some(901));
    store.records.lock().unwrap().push(peer.clone());
    identity.identities.lock().unwrap().remove(&901);

    assert_eq!(session.begin_probe("/bin/agy", None).await.unwrap(), 10);
    session.finish_probe(true, false, false).await;
    assert_eq!(launcher.launched.lock().unwrap().len(), 1);
    assert_eq!(identity.terminations.lock().unwrap().as_slice(), &[peer]);
    assert_eq!(
        store
            .snapshot()
            .into_iter()
            .map(|record| record.pid)
            .collect::<Vec<_>>(),
        vec![10]
    );
    session.reset().await;
}

// upstream: AntigravityCLISessionTests.swift:1142 session reaps stale owner before launch
#[tokio::test]
async fn stale_owner_is_reaped_before_launch() {
    let stale = record(777, "/bin/agy", 42.0, Some(900));
    let store = MemoryStore::with([stale.clone()]);
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    identity.set(777, "/bin/agy", 42.0);
    let session = coordinated_session(
        FakeLauncher::new(),
        Arc::clone(&store),
        Arc::clone(&identity),
    );
    assert_eq!(session.begin_probe("/bin/agy", None).await.unwrap(), 10);
    assert!(identity.terminations.lock().unwrap().contains(&stale));
    assert_eq!(
        session
            .pid()
            .await
            .expect("replacement process should be live"),
        10
    );
    let replacement = store
        .snapshot()
        .into_iter()
        .find(|candidate| candidate.pid == 10)
        .expect("replacement ownership record");
    assert_eq!(replacement.owner_pid, Some(901));
    session.finish_probe(true, true, false).await;
}

// upstream: AntigravityCLISessionTests.swift:1178 reset rechecks protected persisted session after owner exits
#[tokio::test]
async fn reset_rechecks_protected_session_after_owner_exits() {
    let protected = record(777, "/bin/agy", 42.0, Some(900));
    let store = MemoryStore::with([protected.clone()]);
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    identity.set(777, "/bin/agy", 42.0);
    identity.set(900, "/app/ariadusage", 1.0);
    let session = coordinated_session(
        FakeLauncher::new(),
        Arc::clone(&store),
        Arc::clone(&identity),
    );
    session.reset().await;
    identity.identities.lock().unwrap().remove(&900);
    session.reset().await;
    assert!(store.snapshot().is_empty());
    assert!(identity.terminations.lock().unwrap().contains(&protected));
}

// upstream: AntigravityCLISessionTests.swift:1206 teardown preserves record written by another live session
#[tokio::test]
async fn teardown_preserves_record_written_by_another_session() {
    let store = MemoryStore::with(std::iter::empty::<SessionRecord>());
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    let launcher = FakeLauncher::new();
    let session = coordinated_session(
        Arc::clone(&launcher),
        Arc::clone(&store),
        Arc::clone(&identity),
    );
    session.begin_probe("/bin/agy", None).await.unwrap();
    let other = record(777, "/other/agy", 42.0, None);
    store.records.lock().unwrap().push(other.clone());
    session.finish_probe(true, true, false).await;
    assert_eq!(store.snapshot(), vec![other]);
}

// upstream: AntigravityCLISessionTests.swift:885 active probe prevents idle teardown until finish
#[tokio::test]
async fn active_probe_prevents_teardown_until_finish() {
    let launcher = FakeLauncher::new();
    let clock = Arc::new(ManualClock::default());
    let mut dependencies = Dependencies::new(Arc::clone(&launcher) as Arc<dyn ProcessLauncher>);
    dependencies.clock = Arc::clone(&clock) as Arc<dyn SessionClock>;
    dependencies.idle_window = Duration::from_millis(50);
    let session = AntigravityCLISession::new(dependencies);
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(true, false, false).await;
    session.begin_probe("/bin/agy", None).await.unwrap();
    clock.wait_for_sleeps(1).await;
    clock.resume_all();
    tokio::task::yield_now().await;
    assert!(session.is_running().await);
    session.finish_probe(true, false, false).await;
    clock.wait_for_sleeps(1).await;
    clock.resume_all();
    wait_until_stopped(&session).await;
}

// upstream: AntigravityCLISessionTests.swift:905 manual reset waits for active probe to finish
#[tokio::test]
async fn manual_reset_waits_for_active_probe() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.reset().await;
    assert!(session.is_running().await);
    session.finish_probe(true, false, false).await;
    assert!(!session.is_running().await);
}

// upstream: AntigravityCLISessionTests.swift:874 host idle window extends the default session lifetime
#[tokio::test]
async fn host_idle_window_extends_default_lifetime() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session
        .begin_probe("/bin/agy", Some(Duration::from_secs(360)))
        .await
        .unwrap();
    session.finish_probe(true, false, false).await;
    assert_eq!(session.idle_window().await, Duration::from_secs(360));
    session.reset().await;
}

// upstream: AntigravityCLISessionTests.swift:858 idle window tears down warm process
#[tokio::test]
async fn idle_window_tears_down_warm_process() {
    let launcher = FakeLauncher::new();
    let store = MemoryStore::with(std::iter::empty::<SessionRecord>());
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    let clock = Arc::new(ManualClock::default());
    let mut dependencies = Dependencies::new(Arc::clone(&launcher) as Arc<dyn ProcessLauncher>);
    dependencies.clock = Arc::clone(&clock) as Arc<dyn SessionClock>;
    dependencies.idle_window = Duration::from_millis(50);
    dependencies.record_store = Some(Arc::clone(&store) as Arc<dyn SessionRecordStore>);
    dependencies.identity_provider = Some(identity);
    dependencies.launch_lock = Some(Arc::new(FakeLock::default()));
    let session = AntigravityCLISession::new(dependencies);
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(true, false, false).await;
    clock.wait_for_sleeps(1).await;
    clock.resume_all();
    wait_until_stopped(&session).await;
    assert!(store.snapshot().is_empty());
}

// upstream: AntigravityCLISessionTests.swift:773 authentication prompt matcher tolerates prompt casing and spacing
#[test]
fn authentication_prompt_tolerates_spacing_and_case() {
    assert!(AuthenticationPrompt::contains(b"select  LOGIN\nmethod :"));
    assert!(AuthenticationPrompt::contains(b"Select login method:"));
    assert!(!AuthenticationPrompt::contains(
        b"You are currently not signed in"
    ));
}

// upstream: AntigravityCLISessionTests.swift:783 authentication prompt matcher recognizes logged out and exhausted keyring states
#[test]
fn authentication_prompt_recognizes_logged_out_and_keyring_states() {
    assert!(AuthenticationPrompt::contains(
        b"You are not logged into Antigravity."
    ));
    assert!(AuthenticationPrompt::contains(
        b"keyringAuth: timed out after 10s, skipping keyring auth"
    ));
    assert!(AuthenticationPrompt::contains(
        b"KEYRING auth : timed out after 10s"
    ));
    assert!(!AuthenticationPrompt::contains(
        b"Welcome. You are currently not signed in.\nSigning in..."
    ));
}

// upstream: AntigravityCLISessionTests.swift:795 session returns complete new output before retaining only its tail
#[tokio::test]
async fn drain_returns_new_output_and_retains_auth_evidence() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/bin/agy", None).await.unwrap();
    let oversized = [b"Select login method:".to_vec(), vec![b' '; 8192]].concat();
    launcher.processes.lock().unwrap()[0]
        .output
        .lock()
        .unwrap()
        .push_back(oversized.clone());
    let output = session.drain_output().await;
    assert_eq!(output, oversized);
    assert!(AuthenticationPrompt::contains(&output));
    assert!(AuthenticationPrompt::contains(
        &session.drain_output().await
    ));
    session.finish_probe(false, true, true).await;
}

struct FailingLauncher;

impl ProcessLauncher for FailingLauncher {
    fn launch(&self, _binary: &Path) -> ProcessFuture<Result<Arc<dyn ProcessHandle>, String>> {
        Box::pin(async { Err("launch rejected".to_owned()) })
    }
}

// MISSING upstream AntigravityCLISessionTests.swift:816: ProcessLauncher launch errors do not
// expose a launched handle, and ProcessRegistry rejects fenced launches before spawning.
#[tokio::test]
async fn launch_failure_is_returned_without_a_live_session() {
    let session = AntigravityCLISession::new(Dependencies::new(
        Arc::new(FailingLauncher) as Arc<dyn ProcessLauncher>
    ));
    assert!(session.begin_probe("/bin/agy", None).await.is_err());
    assert!(!session.is_running().await);
}

// upstream: AntigravityCLISessionTests.swift:833 launch remains usable when coordination lock is unavailable
#[tokio::test]
async fn unavailable_coordination_lock_does_not_block_launch() {
    let launcher = FakeLauncher::new();
    let store = MemoryStore::with(std::iter::empty::<SessionRecord>());
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    let mut dependencies = Dependencies::new(Arc::clone(&launcher) as Arc<dyn ProcessLauncher>);
    dependencies.record_store = Some(Arc::clone(&store) as Arc<dyn SessionRecordStore>);
    dependencies.identity_provider = Some(identity);
    dependencies.launch_lock = Some(Arc::new(FakeLock {
        held: Mutex::new(true),
    }));
    let session = AntigravityCLISession::new(dependencies);
    assert_eq!(session.begin_probe("/bin/agy", None).await.unwrap(), 10);
    session.finish_probe(true, true, false).await;
    assert!(store.snapshot().is_empty());
}

// upstream: AntigravityCLISessionTests.swift:845 launch remains usable when ownership record cannot be saved
#[tokio::test]
async fn ownership_record_save_failure_does_not_block_launch() {
    let launcher = FakeLauncher::new();
    let store = Arc::new(MemoryStore {
        records: Mutex::new(Vec::new()),
        fail_save: true,
    });
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    let session = coordinated_session(Arc::clone(&launcher), Arc::clone(&store), identity);
    assert_eq!(session.begin_probe("/bin/agy", None).await.unwrap(), 10);
    session.finish_probe(true, true, false).await;
    assert!(store.snapshot().is_empty());
}

// upstream: AntigravityCLISessionTests.swift:1413 matching persisted stale process is reaped when resolved binary changed
#[tokio::test]
async fn matching_stale_process_is_reaped_when_binary_changes() {
    let stale = record(777, "/old/agy", 42.0, None);
    let store = MemoryStore::with([stale.clone()]);
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    identity.set(777, "/old/agy", 42.0);
    let session = coordinated_session(
        FakeLauncher::new(),
        Arc::clone(&store),
        Arc::clone(&identity),
    );
    session.begin_probe("/new/agy", None).await.unwrap();
    session.finish_probe(true, false, false).await;
    assert!(identity.terminations.lock().unwrap().contains(&stale));
    session.reset().await;
}

// upstream: AntigravityCLISessionTests.swift:1433 matching persisted stale process is reaped before launch
#[tokio::test]
async fn matching_stale_process_is_reaped_before_launch() {
    let stale = record(777, "/bin/agy", 42.0, None);
    let store = MemoryStore::with([stale.clone()]);
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    identity.set(777, "/bin/agy", 42.0);
    let session = coordinated_session(
        FakeLauncher::new(),
        Arc::clone(&store),
        Arc::clone(&identity),
    );
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(true, false, false).await;
    assert!(identity.terminations.lock().unwrap().contains(&stale));
    session.reset().await;
}

// upstream: AntigravityCLISessionTests.swift:1453 non matching persisted process is not reaped
#[tokio::test]
async fn non_matching_persisted_process_is_not_reaped() {
    let stale = record(777, "/bin/agy", 42.0, None);
    let store = MemoryStore::with([stale]);
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    identity.set(777, "/usr/bin/other", 42.0);
    let session = coordinated_session(
        FakeLauncher::new(),
        Arc::clone(&store),
        Arc::clone(&identity),
    );
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(true, false, false).await;
    assert!(identity.terminations.lock().unwrap().is_empty());
    session.reset().await;
}

#[cfg(unix)]
async fn spawn_broker_pty(args: &[&str]) -> PtySession {
    process_support::require_nextest();
    spawn_broker_pty_command(process_support::command(args).env(ProcessEnv::empty())).await
}

#[cfg(unix)]
async fn spawn_broker_pty_command(
    command: ariadusage_engine::brokers::process::Command,
) -> PtySession {
    PtySession::spawn(
        process_support::call(FetchInteraction::UserInitiated),
        command,
        PtySize::default(),
        &ProcessRegistry::new(),
        &LaunchGate::default(),
    )
    .await
    .unwrap()
}

// upstream: AntigravityCLISessionTests.swift:471 replacement launch waits for in progress teardown
#[tokio::test]
async fn replacement_launch_waits_for_teardown_and_releases_process() {
    let launcher = FakeLauncher::new();
    let session = session(Arc::clone(&launcher));
    session.begin_probe("/old/agy", None).await.unwrap();
    session.finish_probe(true, false, false).await;

    let close_started = Arc::new(Notify::new());
    let close_release = Arc::new(Notify::new());
    let old_process = Arc::clone(&launcher.processes.lock().unwrap()[0]);
    *old_process.close_started.lock().unwrap() = Some(Arc::clone(&close_started));
    *old_process.close_release.lock().unwrap() = Some(Arc::clone(&close_release));

    let first_replacement = {
        let session = session.clone();
        tokio::spawn(async move { session.begin_probe("/new/agy", None).await.unwrap() })
    };
    close_started.notified().await;
    let second_replacement = {
        let session = session.clone();
        tokio::spawn(async move { session.begin_probe("/new/agy", None).await.unwrap() })
    };
    tokio::task::yield_now().await;
    assert_eq!(
        launcher.launched.lock().unwrap().as_slice(),
        [PathBuf::from("/old/agy")]
    );

    close_release.notify_one();
    assert_eq!(first_replacement.await.unwrap(), 11);
    assert_eq!(second_replacement.await.unwrap(), 11);
    session.finish_probe(true, false, false).await;
    session.finish_probe(true, false, false).await;
    assert_eq!(
        launcher.launched.lock().unwrap().as_slice(),
        [PathBuf::from("/old/agy"), PathBuf::from("/new/agy")]
    );
    session.reset().await;
}

#[cfg(unix)]
// upstream: AntigravityCLISessionTests.swift:613 pty launcher creates dedicated process group before returning
#[tokio::test]
async fn broker_pty_creates_a_dedicated_process_group() {
    let mut session = spawn_broker_pty(&["pty-prompt", "idle"]).await;
    let pid = session.pid().expect("PTY root PID");
    assert_eq!(session.process_group(), Some(pid));
    assert_eq!(
        ariadusage_engine::brokers::process::process_group(
            Path::new("/proc"),
            i32::try_from(pid).unwrap()
        ),
        Some(i32::try_from(pid).unwrap())
    );
    session.close().await;
}

#[cfg(unix)]
// upstream: AntigravityCLISessionTests.swift:1224 force killed process is polled again so the child can be reaped
#[tokio::test]
async fn broker_force_termination_reaps_the_owned_group() {
    let mut session = spawn_broker_pty(&["pty-prompt", "idle"]).await;
    assert!(session.pid().is_some());
    session.terminate().await;
    assert!(!session.is_running());
}

// MISSING upstream AntigravityCLISessionTests.swift:627: the broker has no safe PTY
// signal-mask test hook for SIGINT, SIGTERM, and SIGHUP.

#[cfg(unix)]
// HOME/environment broker adaptation; the upstream FD-closing case remains MISSING.
#[tokio::test]
async fn broker_pty_applies_environment_and_closes_cleanly() {
    assert_pty_home_and_environment().await;
}

#[cfg(unix)]
async fn assert_pty_home_and_environment() {
    let home_dir = tempfile::tempdir().unwrap();
    let home = home_dir.path().canonicalize().unwrap();
    let mut command = process_support::command(&["pty-prompt", "env"]).env(
        ProcessEnv::empty()
            .with("HOME", home.as_os_str())
            .with("UNLISTED_PROCESS_TEST", "present"),
    );
    command.set_cwd(Some(home.to_path_buf()));
    let mut session = spawn_broker_pty_command(command).await;
    let output = session
        .run_script(ariadusage_engine::brokers::process::PtyScript::new(
            Vec::new(),
            Duration::from_secs(2),
        ))
        .await
        .unwrap();
    let home = home.to_string_lossy();
    assert!(output.text.contains(&format!("HOME={home}")));
    assert!(output.text.contains(&format!("PWD={home}")));
    assert!(output.text.contains(&format!("CWD={home}")));
    assert!(output.text.contains("UNLISTED_PROCESS_TEST=present"));
    session.close().await;
}

#[cfg(all(unix, feature = "test-hooks"))]
// upstream: AntigravityCLISessionTests.swift:720 spawned PTY drain is bounded per call
#[tokio::test]
async fn broker_pty_drain_is_bounded_per_call() {
    let source = vec![1_u8; 8192 * 65];
    let (output, offset) =
        ariadusage_engine::brokers::process::drain_bounded_for_test(source.clone())
            .await
            .unwrap();
    assert_eq!(output, source[..8192 * 64]);
    assert_eq!(offset, 8192 * 64);
}

#[cfg(all(unix, feature = "test-hooks"))]
// upstream: AntigravityCLISessionTests.swift:636 pty launcher retries transient text busy spawn errors
#[tokio::test]
async fn broker_retries_transient_text_busy_spawn_errors() {
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&attempts);
    let result = ariadusage_engine::brokers::process::retry_spawn_for_test(true, move || {
        let attempt = observed.fetch_add(1, Ordering::SeqCst);
        if attempt < 2 {
            Err(io::Error::from_raw_os_error(26))
        } else {
            Ok("spawned")
        }
    })
    .await
    .unwrap();
    assert_eq!(result, "spawned");
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

#[cfg(all(unix, feature = "test-hooks"))]
// upstream: AntigravityCLISessionTests.swift:649 pty launcher does not retry other spawn errors
#[tokio::test]
async fn broker_does_not_retry_non_text_busy_spawn_errors() {
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&attempts);
    assert!(
        ariadusage_engine::brokers::process::retry_spawn_for_test(false, move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(io::Error::from_raw_os_error(13))
        })
        .await
        .is_err()
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

// upstream: AntigravityCLISessionTests.swift:1317 idle timeout hard stops a previously failed process
#[tokio::test]
async fn idle_timeout_hard_stops_a_failed_process() {
    let launcher = FakeLauncher::new();
    let clock = Arc::new(ManualClock::default());
    let mut dependencies = Dependencies::new(Arc::clone(&launcher) as Arc<dyn ProcessLauncher>);
    dependencies.clock = Arc::clone(&clock) as Arc<dyn SessionClock>;
    dependencies.idle_window = Duration::from_millis(50);
    let session = AntigravityCLISession::new(dependencies);
    session.begin_probe("/bin/agy", None).await.unwrap();
    session.finish_probe(false, false, false).await;
    clock.wait_for_sleeps(1).await;
    clock.resume_all();
    wait_until_stopped(&session).await;
    assert_eq!(
        launcher.processes.lock().unwrap()[0].events(),
        vec!["force"]
    );
}

#[tokio::test]
async fn persisted_record_uses_the_verified_identity_provider() {
    let store = MemoryStore::with(std::iter::empty::<SessionRecord>());
    let identity = FakeIdentity::current(901, "/app/ariadusage", 1.0);
    identity.set(10, "/resolved/agy", 100.0);
    let session = coordinated_session(FakeLauncher::new(), store.clone(), identity);
    session.begin_probe("/selected/agy", None).await.unwrap();
    let records = store.snapshot();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].executable_path, "/resolved/agy");
    assert_eq!(records[0].start_epoch, 100.0);
    session.finish_probe(true, true, false).await;
}
