// Ported from CodexBar Sources/CodexBarCore/Host/Process/SpawnedProcessGroup.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Host/Process/ProcessOwnershipReaper.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

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
    descendants: Arc<Mutex<HashMap<i32, ProcessIdentity>>>,
}

impl Clone for ProcessTarget {
    fn clone(&self) -> Self {
        Self {
            root: self.root,
            pgid: self.pgid,
            uid: self.uid,
            pipes: self.pipes.clone(),
            descendants: Arc::clone(&self.descendants),
        }
    }
}

impl ProcessTarget {
    pub fn new(root: ProcessIdentity, pgid: i32, uid: u32, pipes: Vec<PathBuf>) -> Self {
        Self {
            root,
            pgid,
            uid,
            pipes,
            descendants: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn refresh_descendants(&self) {
        if !identity_is_current(self.root, self.uid) {
            return;
        }
        let root = Path::new("/proc");
        let mut known = lock(&self.descendants);
        for identity in process_descendants(root, self.root, self.uid) {
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
    let descendants = current_descendants(target, root);
    signal_identities(descendants, ProcessSignal::Term);
    signal_target_group(target, ProcessSignal::Term);
    if identity_is_current(target.root, target.uid) {
        let _ = signal(target.root, ProcessSignal::Term);
    }

    let deadline = tokio::time::Instant::now() + TEARDOWN_GRACE;
    while tokio::time::Instant::now() < deadline && targets_running(target, root) {
        target.refresh_descendants();
        tokio::time::sleep(TEARDOWN_POLL).await;
    }

    target.refresh_descendants();
    signal_identities(current_descendants(target, root), ProcessSignal::Kill);
    signal_target_group(target, ProcessSignal::Kill);
    if identity_is_current(target.root, target.uid) {
        let _ = signal(target.root, ProcessSignal::Kill);
    }
}

pub(super) fn terminate_sync(target: &ProcessTarget) {
    let root = Path::new("/proc");
    target.refresh_descendants();
    signal_identities(current_descendants(target, root), ProcessSignal::Term);
    signal_target_group(target, ProcessSignal::Term);
    if identity_is_current(target.root, target.uid) {
        let _ = signal(target.root, ProcessSignal::Term);
    }

    let deadline = std::time::Instant::now() + TEARDOWN_GRACE;
    while std::time::Instant::now() < deadline && targets_running(target, root) {
        target.refresh_descendants();
        std::thread::sleep(TEARDOWN_POLL);
    }

    target.refresh_descendants();
    signal_identities(current_descendants(target, root), ProcessSignal::Kill);
    signal_target_group(target, ProcessSignal::Kill);
    if identity_is_current(target.root, target.uid) {
        let _ = signal(target.root, ProcessSignal::Kill);
    }
}

fn current_descendants(target: &ProcessTarget, root: &Path) -> Vec<ProcessIdentity> {
    let mut candidates = target.descendant_snapshot();
    if identity_is_current(target.root, target.uid) {
        candidates.extend(process_descendants(root, target.root, target.uid));
    }
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

fn signal_target_group(target: &ProcessTarget, signal_kind: ProcessSignal) {
    let root = Path::new("/proc");
    let witness = std::iter::once(target.root)
        .chain(target.descendant_snapshot())
        .chain(group_members(target, root))
        .find(|identity| {
            identity_is_current(*identity, target.uid)
                && process_group(root, identity.pid) == Some(target.pgid)
        });
    if let Some(witness) = witness {
        let _ = signal_group(witness, target.pgid, signal_kind);
    }
}

fn signal_identities(identities: Vec<ProcessIdentity>, signal_kind: ProcessSignal) {
    for identity in identities {
        let _ = signal(identity, signal_kind);
    }
}

fn targets_running(target: &ProcessTarget, root: &Path) -> bool {
    std::iter::once(target.root)
        .chain(current_descendants(target, root))
        .chain(group_members(target, root))
        .any(|identity| {
            identity_is_current(identity, target.uid)
                && process_state(root, identity.pid).is_some_and(|state| state != 'Z')
        })
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
