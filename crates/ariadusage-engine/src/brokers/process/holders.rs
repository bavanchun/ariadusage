// Ported from CodexBar Sources/CodexBarCore/Host/Process/SpawnedProcessGroup.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::procscan::{
    ProcessIdentity, fd_targets, process_identity, process_uid, same_uid_processes,
};
use super::signal::{ProcessSignal, signal_if};
use super::teardown::{ProcessTarget, TEARDOWN_GRACE};

const HOLDER_POLL: Duration = Duration::from_millis(20);

pub(super) async fn reap_output_holders(target: &ProcessTarget) {
    if target.pipes.is_empty() {
        return;
    }
    let root = Path::new("/proc");
    let first = output_holders(target, root);
    signal_holders(target, &first, ProcessSignal::Term);

    let deadline = tokio::time::Instant::now() + TEARDOWN_GRACE;
    while tokio::time::Instant::now() < deadline && !output_holders(target, root).is_empty() {
        tokio::time::sleep(HOLDER_POLL).await;
    }

    let remaining = output_holders(target, root);
    signal_holders(target, &remaining, ProcessSignal::Kill);
}

pub(super) fn reap_output_holders_sync(target: &ProcessTarget) {
    if target.pipes.is_empty() {
        return;
    }
    let root = Path::new("/proc");
    let first = output_holders(target, root);
    signal_holders(target, &first, ProcessSignal::Term);

    let deadline = std::time::Instant::now() + TEARDOWN_GRACE;
    while std::time::Instant::now() < deadline && !output_holders(target, root).is_empty() {
        std::thread::sleep(HOLDER_POLL);
    }

    let remaining = output_holders(target, root);
    signal_holders(target, &remaining, ProcessSignal::Kill);
}

fn output_holders(target: &ProcessTarget, root: &Path) -> Vec<ProcessIdentity> {
    same_uid_processes(root, target.uid)
        .into_iter()
        .filter(|identity| identity.pid != target.root.pid)
        .filter(|identity| holds_target_pipe(root, *identity, target.uid, &target.pipes))
        .collect()
}

fn signal_holders(
    target: &ProcessTarget,
    identities: &[ProcessIdentity],
    signal_kind: ProcessSignal,
) {
    let root = Path::new("/proc");
    signal_holders_with(
        root,
        identities,
        target.uid,
        &target.pipes,
        signal_kind,
        |identity, kind| {
            let _ = signal_if(identity, kind, || {
                holds_target_pipe(root, identity, target.uid, &target.pipes)
            });
        },
    );
}

fn signal_holders_with(
    root: &Path,
    identities: &[ProcessIdentity],
    uid: u32,
    pipes: &[PathBuf],
    signal_kind: ProcessSignal,
    mut send: impl FnMut(ProcessIdentity, ProcessSignal),
) {
    for identity in identities {
        if holds_target_pipe(root, *identity, uid, pipes) {
            send(*identity, signal_kind);
        }
    }
}

fn holds_target_pipe(root: &Path, identity: ProcessIdentity, uid: u32, pipes: &[PathBuf]) -> bool {
    identity.pid != std::process::id() as i32
        && process_identity(root, identity.pid) == Some(identity)
        && process_uid(root, identity.pid) == Some(uid)
        && fd_targets(root, identity.pid)
            .iter()
            .any(|fd_target| pipes.contains(fd_target))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{ProcessIdentity, ProcessSignal, holds_target_pipe, signal_holders_with};

    #[test]
    fn holder_that_closes_its_pipe_after_discovery_is_not_escalated() {
        let proc_root = tempfile::tempdir().unwrap();
        let root = proc_root.path();
        let uid = rustix::process::getuid().as_raw();
        let identity = ProcessIdentity {
            pid: 500,
            start_ticks: 5_000,
        };
        let pipe = PathBuf::from("pipe:[synthetic-123]");
        write_fake_process(root, identity, uid, &pipe);
        assert!(holds_target_pipe(
            root,
            identity,
            uid,
            std::slice::from_ref(&pipe)
        ));

        std::fs::remove_file(root.join("500/fd/1")).unwrap();
        std::os::unix::fs::symlink("pipe:[synthetic-456]", root.join("500/fd/1")).unwrap();
        let mut sent = Vec::new();
        signal_holders_with(
            root,
            &[identity],
            uid,
            std::slice::from_ref(&pipe),
            ProcessSignal::Kill,
            |identity, signal| sent.push((identity, signal)),
        );

        assert!(sent.is_empty());
    }

    fn write_fake_process(root: &Path, identity: ProcessIdentity, uid: u32, pipe: &Path) {
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
        let fd = process.join("fd");
        std::fs::create_dir_all(&fd).unwrap();
        std::os::unix::fs::symlink(pipe, fd.join("1")).unwrap();
    }
}
