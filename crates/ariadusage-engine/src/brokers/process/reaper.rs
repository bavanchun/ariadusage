// Ported from CodexBar Sources/CodexBarCore/Host/Process/ProcessOwnershipReaper.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/ProcessOwnershipReaperTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::path::Path;
use std::time::Duration;

use super::procscan::{
    PROCESS_MARKER_ENV, ProcessIdentity, process_group, process_identity, process_uid,
    read_marker_environment, same_uid_processes,
};
use super::signal::{ProcessSignal, signal_group_if, signal_if};

const REAPER_GRACE: Duration = Duration::from_millis(400);
const REAPER_INTERVAL: Duration = Duration::from_millis(50);

pub(super) struct ReapGuard {
    marker: Option<String>,
    pgid: i32,
    active: bool,
}

impl ReapGuard {
    pub fn new(marker: Option<String>, pgid: i32) -> Self {
        Self {
            active: marker.is_some(),
            marker,
            pgid,
        }
    }

    pub fn set_pgid(&mut self, pgid: i32) {
        self.pgid = pgid;
    }

    pub async fn finish(&mut self) {
        if let Some(marker) = self.marker.as_deref() {
            reap_marker(marker.as_bytes(), self.pgid).await;
        }
        self.active = false;
    }
}

impl Drop for ReapGuard {
    fn drop(&mut self) {
        if self.active
            && let Some(marker) = self.marker.as_deref()
        {
            reap_marker_sync(marker.as_bytes(), self.pgid);
        }
    }
}

pub(super) async fn reap_marker(marker: &[u8], pgid: i32) {
    let root = Path::new("/proc");
    signal_marked_group(root, marker, pgid, ProcessSignal::Term);
    let deadline = tokio::time::Instant::now() + REAPER_GRACE;
    loop {
        let owned = marked_processes(root, marker);
        if owned.is_empty() {
            break;
        }
        signal_identities(root, &owned, marker, ProcessSignal::Term);
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(REAPER_INTERVAL).await;
    }
    signal_marked_group(root, marker, pgid, ProcessSignal::Kill);
    let owned = marked_processes(root, marker);
    signal_identities(root, &owned, marker, ProcessSignal::Kill);
}

pub(super) fn reap_marker_sync(marker: &[u8], pgid: i32) {
    let root = Path::new("/proc");
    signal_marked_group(root, marker, pgid, ProcessSignal::Term);
    let deadline = std::time::Instant::now() + REAPER_GRACE;
    loop {
        let owned = marked_processes(root, marker);
        if owned.is_empty() {
            break;
        }
        signal_identities(root, &owned, marker, ProcessSignal::Term);
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(REAPER_INTERVAL);
    }
    signal_marked_group(root, marker, pgid, ProcessSignal::Kill);
    let owned = marked_processes(root, marker);
    signal_identities(root, &owned, marker, ProcessSignal::Kill);
}

fn marked_processes(root: &Path, marker: &[u8]) -> Vec<ProcessIdentity> {
    let uid = rustix::process::getuid().as_raw();
    same_uid_processes(root, uid)
        .into_iter()
        .filter(|identity| marker_is_current(root, *identity, uid, marker))
        .collect()
}

fn signal_marked_group(root: &Path, marker: &[u8], pgid: i32, signal_kind: ProcessSignal) {
    if pgid <= 1 {
        return;
    }
    let uid = rustix::process::getuid().as_raw();
    for identity in marked_processes(root, marker) {
        if process_group(root, identity.pid) == Some(pgid)
            && signal_group_if(identity, pgid, signal_kind, || {
                marker_is_current(root, identity, uid, marker)
            })
            .unwrap_or(false)
        {
            return;
        }
    }
}

fn signal_identities(
    root: &Path,
    identities: &[ProcessIdentity],
    marker: &[u8],
    signal_kind: ProcessSignal,
) {
    let uid = rustix::process::getuid().as_raw();
    signal_marked_identities(
        root,
        identities,
        uid,
        marker,
        signal_kind,
        |identity, kind| {
            let _ = signal_if(identity, kind, || {
                marker_is_current(root, identity, uid, marker)
            });
        },
    );
}

fn signal_marked_identities(
    root: &Path,
    identities: &[ProcessIdentity],
    uid: u32,
    marker: &[u8],
    signal_kind: ProcessSignal,
    mut send: impl FnMut(ProcessIdentity, ProcessSignal),
) {
    for identity in identities {
        if marker_is_current(root, *identity, uid, marker) {
            send(*identity, signal_kind);
        }
    }
}

fn marker_is_current(root: &Path, identity: ProcessIdentity, uid: u32, marker: &[u8]) -> bool {
    process_identity(root, identity.pid) == Some(identity)
        && process_uid(root, identity.pid) == Some(uid)
        && read_marker_environment(root, identity.pid, PROCESS_MARKER_ENV)
            .is_some_and(|current| current.as_slice() == marker)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ProcessIdentity, ProcessSignal, marker_is_current, signal_marked_identities};

    #[test]
    // CodexBar: ProcessOwnershipReaperTests.swift:147
    fn marker_lost_after_discovery_is_rejected_before_escalation() {
        let proc_root = tempfile::tempdir().unwrap();
        let root = proc_root.path();
        let uid = rustix::process::getuid().as_raw();
        let marker = b"synthetic-marker";
        let identity = ProcessIdentity {
            pid: 500,
            start_ticks: 5_000,
        };
        write_fake_process(root, identity, uid, marker);
        assert!(marker_is_current(root, identity, uid, marker));

        std::fs::write(root.join("500/environ"), b"").unwrap();
        let mut sent = Vec::new();
        signal_marked_identities(
            root,
            &[identity],
            uid,
            marker,
            ProcessSignal::Kill,
            |identity, signal| sent.push((identity, signal)),
        );

        assert!(sent.is_empty());
    }

    fn write_fake_process(root: &Path, identity: ProcessIdentity, uid: u32, marker: &[u8]) {
        let process = root.join(identity.pid.to_string());
        let task = process.join("task").join(identity.pid.to_string());
        std::fs::create_dir_all(&task).unwrap();
        std::fs::write(task.join("children"), b"").unwrap();
        let mut fields = vec!["0".to_owned(); 50];
        fields[0] = "S".to_owned();
        fields[1] = "1".to_owned();
        fields[2] = identity.pid.to_string();
        fields[19] = identity.start_ticks.to_string();
        std::fs::write(
            process.join("stat"),
            format!(
                "{} (synthetic process) {}\n",
                identity.pid,
                fields.join(" ")
            ),
        )
        .unwrap();
        std::fs::write(
            process.join("status"),
            format!("Name:\tsynthetic\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        )
        .unwrap();
        std::fs::write(
            process.join("environ"),
            [b"ARIADUSAGE_PROCESS_MARKER=".as_slice(), marker, b"\0"].concat(),
        )
        .unwrap();
    }
}
