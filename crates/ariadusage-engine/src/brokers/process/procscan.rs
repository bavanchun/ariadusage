use std::io::Read;
use std::path::{Path, PathBuf};
use std::{collections::BTreeMap, collections::BTreeSet, fs};

use zeroize::Zeroizing;

pub const MAX_ENVIRONMENT_BYTES: usize = 1024 * 1024;
#[cfg(target_os = "linux")]
pub const PROCESS_MARKER_ENV: &str = "ARIADUSAGE_PROCESS_MARKER";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessIdentity {
    pub pid: i32,
    pub start_ticks: u64,
}

pub fn same_uid_processes(root: &Path, uid: u32) -> Vec<ProcessIdentity> {
    #[cfg(target_os = "linux")]
    {
        use procfs::process::all_processes_with_root;

        let Ok(processes) = all_processes_with_root(root) else {
            return Vec::new();
        };
        let current_pid = std::process::id() as i32;
        let mut identities = processes
            .filter_map(Result::ok)
            .filter(|process| process.pid != current_pid)
            .filter_map(|process| {
                let status = process.status().ok()?;
                if status.ruid != uid {
                    return None;
                }
                let stat = process.stat().ok()?;
                Some(ProcessIdentity {
                    pid: process.pid,
                    start_ticks: stat.starttime,
                })
            })
            .collect::<Vec<_>>();
        identities.sort_by_key(|identity| identity.pid);
        identities
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, uid);
        Vec::new()
    }
}

pub fn fd_targets(root: &Path, pid: i32) -> Vec<PathBuf> {
    if pid <= 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(root.join(pid.to_string()).join("fd")) else {
        return Vec::new();
    };
    let mut targets = entries
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .collect::<Vec<_>>();
    targets.sort();
    targets
}

pub fn process_identity(root: &Path, pid: i32) -> Option<ProcessIdentity> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    let fields = stat
        .get(stat.rfind(')')?.checked_add(1)?..)?
        .split_whitespace();
    let start_ticks = fields.clone().nth(19)?.parse().ok()?;
    Some(ProcessIdentity { pid, start_ticks })
}

pub fn process_state(root: &Path, pid: i32) -> Option<char> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    stat.get(stat.rfind(')')?.checked_add(1)?..)?
        .split_whitespace()
        .next()?
        .chars()
        .next()
}

pub fn process_group(root: &Path, pid: i32) -> Option<i32> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    let suffix_start = stat.rfind(')')?.checked_add(1)?;
    stat.get(suffix_start..)?
        .split_whitespace()
        .nth(2)?
        .parse()
        .ok()
}

pub fn process_net_ns(root: &Path, pid: i32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        if pid <= 0 {
            return None;
        }
        std::fs::read_link(root.join(pid.to_string()).join("ns").join("net"))
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, pid);
        None
    }
}

pub fn process_children(root: &Path, pid: i32) -> Vec<i32> {
    if pid <= 0 {
        return Vec::new();
    }
    let task_root = root.join(pid.to_string()).join("task");
    let Ok(tasks) = fs::read_dir(task_root) else {
        return Vec::new();
    };
    let mut children = BTreeSet::new();
    for task in tasks.filter_map(Result::ok) {
        let Ok(task_id) = task.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Ok(contents) = fs::read_to_string(task.path().join("children")) else {
            continue;
        };
        children.extend(
            contents
                .split_whitespace()
                .filter_map(|value| value.parse::<i32>().ok())
                .filter(|child| *child > 0 && *child != task_id),
        );
    }
    children.into_iter().collect()
}

pub fn process_descendants(
    root: &Path,
    identity: ProcessIdentity,
    uid: u32,
) -> Vec<ProcessIdentity> {
    process_descendants_with_hook(root, identity, uid, || {})
}

fn process_descendants_with_hook(
    root: &Path,
    root_identity: ProcessIdentity,
    uid: u32,
    after_walk: impl FnOnce(),
) -> Vec<ProcessIdentity> {
    if !identity_is_current(root, root_identity, uid) {
        return Vec::new();
    }

    let mut pending = process_children(root, root_identity.pid);
    let mut seen = BTreeSet::new();
    let mut found = BTreeMap::new();
    while let Some(pid) = pending.pop() {
        if !seen.insert(pid) {
            continue;
        }
        let Some(child_identity) = process_identity(root, pid) else {
            continue;
        };
        if process_uid(root, pid) != Some(uid) {
            continue;
        }
        let children = process_children(root, pid);
        if !identity_is_current(root, child_identity, uid) {
            continue;
        }
        if !identity_is_current(root, root_identity, uid) {
            return Vec::new();
        }
        found.insert(pid, child_identity);
        pending.extend(children);
    }

    after_walk();
    if !identity_is_current(root, root_identity, uid) {
        return Vec::new();
    }

    found
        .into_values()
        .filter(|identity| identity_is_current(root, *identity, uid))
        .collect()
}

fn identity_is_current(root: &Path, identity: ProcessIdentity, uid: u32) -> bool {
    process_identity(root, identity.pid) == Some(identity)
        && process_uid(root, identity.pid) == Some(uid)
}

pub fn process_uid(root: &Path, pid: i32) -> Option<u32> {
    if pid <= 0 {
        return None;
    }
    let status = std::fs::read_to_string(root.join(pid.to_string()).join("status")).ok()?;
    status.lines().find_map(|line| {
        let mut values = line.strip_prefix("Uid:")?.split_whitespace();
        values.next()?.parse().ok()
    })
}

pub fn read_marker_environment(root: &Path, pid: i32, key: &str) -> Option<Zeroizing<Vec<u8>>> {
    if pid <= 0 {
        return None;
    }
    let path = root.join(pid.to_string()).join("environ");
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(4096));
    file.take((MAX_ENVIRONMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    parse_marker_environment(&bytes, key)
}

pub fn parse_marker_environment(bytes: &[u8], key: &str) -> Option<Zeroizing<Vec<u8>>> {
    if bytes.len() > MAX_ENVIRONMENT_BYTES || bytes.is_empty() || bytes.last() != Some(&0) {
        return None;
    }
    let key = key.as_bytes();
    let mut marker: Option<Zeroizing<Vec<u8>>> = None;
    for record in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        if record.is_empty() {
            return None;
        }
        let separator = record.iter().position(|byte| *byte == b'=')?;
        if separator == 0 {
            return None;
        }
        if &record[..separator] != key {
            continue;
        }
        let value = &record[separator + 1..];
        if let Some(current) = marker.as_ref()
            && current.as_slice() != value
        {
            return None;
        }
        if marker.is_none() {
            marker = Some(Zeroizing::new(value.to_vec()));
        }
    }
    marker
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{ProcessIdentity, process_descendants_with_hook};

    #[test]
    fn descendant_walk_rejects_a_stale_root_identity() {
        let proc_root = tempfile::tempdir().unwrap();
        let uid = rustix::process::getuid().as_raw();
        write_fake_process(proc_root.path(), 500, 5_000, &[501], uid);
        write_fake_process(proc_root.path(), 501, 5_001, &[], uid);

        let descendants = process_descendants_with_hook(
            proc_root.path(),
            ProcessIdentity {
                pid: 500,
                start_ticks: 4_999,
            },
            uid,
            || {},
        );

        assert!(descendants.is_empty());
    }

    #[test]
    fn descendant_walk_discards_results_if_root_identity_changes_during_scan() {
        let proc_root = tempfile::tempdir().unwrap();
        let uid = rustix::process::getuid().as_raw();
        write_fake_process(proc_root.path(), 500, 5_000, &[501], uid);
        write_fake_process(proc_root.path(), 501, 5_001, &[], uid);

        let descendants = process_descendants_with_hook(
            proc_root.path(),
            ProcessIdentity {
                pid: 500,
                start_ticks: 5_000,
            },
            uid,
            || write_fake_process(proc_root.path(), 500, 6_000, &[], uid),
        );

        assert!(descendants.is_empty());
    }

    fn write_fake_process(
        root: &std::path::Path,
        pid: i32,
        start_ticks: u64,
        children: &[i32],
        uid: u32,
    ) {
        let process = root.join(pid.to_string());
        let task = process.join("task").join(pid.to_string());
        std::fs::create_dir_all(&task).unwrap();
        std::fs::write(
            task.join("children"),
            children
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        )
        .unwrap();
        let mut fields = vec!["0".to_owned(); 50];
        fields[0] = "S".to_owned();
        fields[1] = "1".to_owned();
        fields[2] = pid.to_string();
        fields[19] = start_ticks.to_string();
        std::fs::write(
            process.join("stat"),
            format!("{pid} (synthetic process) {}\n", fields.join(" ")),
        )
        .unwrap();
        std::fs::write(
            process.join("status"),
            format!("Name:\tsynthetic\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        )
        .unwrap();
    }
}
