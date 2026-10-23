// Ported from CodexBar Sources/CodexBarCore/Host/Process/SpawnedProcessGroup.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Host/Process/ProcessOwnershipReaper.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use super::procscan::{
    ProcessIdentity, process_descendants, process_group, process_identity, process_state,
    process_uid, same_uid_processes,
};
use super::signal::{ProcessSignal, signal, signal_group};

pub(super) const TEARDOWN_GRACE: Duration = Duration::from_millis(400);
const TEARDOWN_POLL: Duration = Duration::from_millis(20);

pub(super) struct ProcessTarget {
    pub root: ProcessIdentity,
    pub pgid: i32,
    pub uid: u32,
    pub pipes: Vec<PathBuf>,
    pipe_holds: Arc<Mutex<Vec<std::os::fd::OwnedFd>>>,
    descendants: Arc<Mutex<HashMap<i32, ProcessIdentity>>>,
}

impl Clone for ProcessTarget {
    fn clone(&self) -> Self {
        Self {
            root: self.root,
            pgid: self.pgid,
            uid: self.uid,
            pipes: self.pipes.clone(),
            pipe_holds: Arc::clone(&self.pipe_holds),
            descendants: Arc::clone(&self.descendants),
        }
    }
}

impl ProcessTarget {
    pub fn new(root: ProcessIdentity, pgid: i32, uid: u32, pipes: Vec<PathBuf>) -> Self {
        Self::with_pipe_holds(root, pgid, uid, pipes, Vec::new())
    }

    pub fn with_pipe_holds(
        root: ProcessIdentity,
        pgid: i32,
        uid: u32,
        pipes: Vec<PathBuf>,
        pipe_holds: Vec<std::os::fd::OwnedFd>,
    ) -> Self {
        Self {
            root,
            pgid,
            uid,
            pipes,
            pipe_holds: Arc::new(Mutex::new(pipe_holds)),
            descendants: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn release_pipes(&self) {
        lock(&self.pipe_holds).clear();
    }

    pub fn refresh_descendants(&self) {
        if !identity_is_current(self.root, self.uid) {
            return;
        }
        let root = Path::new("/proc");
        let found = process_descendants(root, self.root, self.uid);
        let mut known = lock(&self.descendants);
        for identity in found {
            known.insert(identity.pid, identity);
        }
    }

    fn descendant_snapshot(&self) -> Vec<ProcessIdentity> {
        lock(&self.descendants).values().copied().collect()
    }
}

pub(super) async fn terminate(target: &ProcessTarget) {
    let root = Path::new("/proc");
    target.refresh_descendants();
    let descendants = current_descendants(target);
    let root_alive = identity_is_current(target.root, target.uid);
    signal_identities(descendants.clone(), ProcessSignal::Term);
    let group_signaled = signal_target_group(target, ProcessSignal::Term);
    if root_alive {
        let _ = signal(target.root, ProcessSignal::Term);
    }
    if !root_alive && descendants.is_empty() && !group_signaled {
        return;
    }

    let deadline = tokio::time::Instant::now() + TEARDOWN_GRACE;
    let mut still_running = targets_running(target, root);
    while still_running && tokio::time::Instant::now() < deadline {
        target.refresh_descendants();
        tokio::time::sleep(TEARDOWN_POLL).await;
        still_running = targets_running(target, root);
    }

    if still_running {
        target.refresh_descendants();
        signal_identities(current_descendants(target), ProcessSignal::Kill);
        let _ = signal_target_group(target, ProcessSignal::Kill);
        if identity_is_current(target.root, target.uid) {
            let _ = signal(target.root, ProcessSignal::Kill);
        }
    }
}

pub(super) fn terminate_sync(target: &ProcessTarget) {
    let root = Path::new("/proc");
    target.refresh_descendants();
    let descendants = current_descendants(target);
    let root_alive = identity_is_current(target.root, target.uid);
    signal_identities(descendants.clone(), ProcessSignal::Term);
    let group_signaled = signal_target_group(target, ProcessSignal::Term);
    if root_alive {
        let _ = signal(target.root, ProcessSignal::Term);
    }
    if !root_alive && descendants.is_empty() && !group_signaled {
        return;
    }

    let deadline = std::time::Instant::now() + TEARDOWN_GRACE;
    let mut still_running = targets_running(target, root);
    while still_running && std::time::Instant::now() < deadline {
        target.refresh_descendants();
        std::thread::sleep(TEARDOWN_POLL);
        still_running = targets_running(target, root);
    }

    if still_running {
        target.refresh_descendants();
        signal_identities(current_descendants(target), ProcessSignal::Kill);
        let _ = signal_target_group(target, ProcessSignal::Kill);
        if identity_is_current(target.root, target.uid) {
            let _ = signal(target.root, ProcessSignal::Kill);
        }
    }
}

fn current_descendants(target: &ProcessTarget) -> Vec<ProcessIdentity> {
    let candidates = target.descendant_snapshot();
    let mut current = HashMap::new();
    for identity in candidates {
        if identity_is_current(identity, target.uid) {
            current.insert(identity.pid, identity);
        }
    }
    let mut identities = current.into_values().collect::<Vec<_>>();
    identities.sort_by_key(|identity| identity.pid);
    identities
}

fn group_members(target: &ProcessTarget, root: &Path) -> Vec<ProcessIdentity> {
    same_uid_processes(root, target.uid)
        .into_iter()
        .filter(|identity| process_group(root, identity.pid) == Some(target.pgid))
        .filter(|identity| identity_is_current(*identity, target.uid))
        .collect()
}

fn signal_target_group(target: &ProcessTarget, signal_kind: ProcessSignal) -> bool {
    let root = Path::new("/proc");
    let is_witness = |identity: ProcessIdentity| {
        identity_is_current(identity, target.uid)
            && process_group(root, identity.pid) == Some(target.pgid)
    };
    let witness = std::iter::once(target.root)
        .chain(target.descendant_snapshot())
        .find(|&identity| is_witness(identity))
        .or_else(|| {
            group_members(target, root)
                .into_iter()
                .find(|&identity| is_witness(identity))
        });
    if let Some(witness) = witness {
        let _ = signal_group(witness, target.pgid, signal_kind);
        true
    } else {
        false
    }
}

fn signal_identities(identities: Vec<ProcessIdentity>, signal_kind: ProcessSignal) {
    for identity in identities {
        let _ = signal(identity, signal_kind);
    }
}

fn targets_running(target: &ProcessTarget, root: &Path) -> bool {
    let is_running = |identity: ProcessIdentity| {
        identity_is_current(identity, target.uid)
            && process_state(root, identity.pid).is_some_and(|state| state != 'Z')
    };
    if is_running(target.root) {
        return true;
    }
    target.refresh_descendants();
    current_descendants(target).into_iter().any(is_running)
        || group_members(target, root).into_iter().any(is_running)
}

fn identity_is_current(identity: ProcessIdentity, uid: u32) -> bool {
    let root = Path::new("/proc");
    process_identity(root, identity.pid) == Some(identity)
        && process_uid(root, identity.pid) == Some(uid)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
